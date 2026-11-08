use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tower_http::{
    compression::{
        predicate::{NotForContentType, Predicate, SizeAbove},
        CompressionLayer,
    },
    cors::CorsLayer,
    services::ServeDir,
};
use tracing::{error, info};

/// Maximum accepted upload size. `axum` defaults to 2 MB, which rejects most real PDFs.
const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// Compiled into the binary so serving the page does not touch the disk.
const INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Serialize)]
struct PageInfoResponse {
    page_count: usize,
}

#[tokio::main]
async fn main() {
    // Initialize subscriber for logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Static assets are wrapped in their own router so they can carry cache headers.
    let static_files = Router::new()
        .nest_service("/", ServeDir::new("static"))
        .layer(middleware::from_fn(static_cache_headers));

    // Build Axum router with permissive CORS layer
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/compress", post(compress_handler))
        .route("/api/split", post(split_handler))
        .route("/api/merge", post(merge_handler))
        .route("/api/rotate", post(rotate_handler))
        .route("/api/organize", post(organize_handler))
        .route("/api/pages-info", post(pages_info_handler))
        .nest("/static", static_files)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(CorsLayer::permissive())
        // Keep it outermost: it only compresses text-like responses, never the PDFs.
        .layer(
            CompressionLayer::new().compress_when(
                SizeAbove::new(1024)
                    .and(NotForContentType::IMAGES)
                    .and(NotForContentType::new("application/pdf")),
            ),
        );

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:3000").await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind to port 3000: {}", e);
            std::process::exit(1);
        }
    };
    info!("🚀 ihatepdf server running at http://127.0.0.1:3000");
    if let Err(e) = axum::serve(listener, app).await {
        error!("Server error: {}", e);
    }
}

async fn static_cache_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    // Only successful responses are worth caching; don't pin 404s for a day.
    if response.status().is_success() {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        );
    }
    response
}

async fn index_handler() -> impl IntoResponse {
    // In release the page is compiled into the binary (no disk I/O). In debug we
    // read it from disk so edits show up without recompiling.
    #[cfg(debug_assertions)]
    let html = tokio::fs::read_to_string("static/index.html")
        .await
        .unwrap_or_else(|_| INDEX_HTML.to_string());
    #[cfg(not(debug_assertions))]
    let html = INDEX_HTML.to_string();

    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
        Html(html),
    )
}

// Handler for Compress PDF
//
// Strategy (all in memory, never returns a bigger file):
//   1. Drop application-private `/PieceInfo` data (Illustrator/InDesign edit
//      data). It is invisible to viewers but can dominate the file size.
//   2. Remove the now-unreachable objects.
//   3. Flate-compress every stream that is still uncompressed.
//   4. If the result is not smaller than the input, return the input as-is.
async fn compress_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            // Surfaces the right status (413 for an oversized upload) instead
            // of silently treating it as a missing file field.
            Err(e) => return e.into_response(),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if let Ok(bytes) = field.bytes().await {
                file_data = Some(bytes.to_vec());
                break;
            }
        }
    }

    let data = match file_data {
        Some(d) => d,
        None => return bad_request("Missing 'file' field"),
    };

    let input_len = data.len();
    let result = tokio::task::spawn_blocking(move || compress_pdf(&data)).await;

    match result {
        Ok(Ok(bytes)) => {
            if bytes.len() < input_len {
                info!(
                    "Compressed {} -> {} bytes ({:.1}% smaller)",
                    input_len,
                    bytes.len(),
                    (1.0 - bytes.len() as f64 / input_len as f64) * 100.0
                );
            }
            send_bytes_response(bytes, "compressed.pdf")
        }
        Ok(Err(e)) => {
            error!("Compression failed: {}", e);
            bad_request(e)
        }
        Err(e) => {
            error!("Compress task failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Loads a PDF in memory, rejecting password-protected files.
///
/// `lopdf` can parse the container of an encrypted PDF but does not decrypt its
/// streams, so rewriting or recompressing one would silently corrupt it. Better
/// to fail with a clear message.
fn load_document(data: &[u8]) -> Result<Document, String> {
    let doc = Document::load_mem(data).map_err(|e| format!("Invalid or unreadable PDF: {}", e))?;
    if doc.is_encrypted() {
        return Err(
            "Password-protected PDFs are not supported. Remove the password and try again"
                .to_string(),
        );
    }
    Ok(doc)
}

/// Compresses a PDF in memory and never returns something larger than the
/// input. When no optimization pays off, the original bytes are returned.
fn compress_pdf(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut doc = load_document(data)?;

    for object in doc.objects.values_mut() {
        strip_private_data(object);
    }
    doc.prune_objects();
    doc.compress();

    let mut buffer = Vec::new();
    doc.save_to(&mut buffer).map_err(|e| e.to_string())?;

    if buffer.len() < data.len() {
        Ok(buffer)
    } else {
        Ok(data.to_vec())
    }
}

/// Recursively removes `/PieceInfo`, the PDF page-piece dictionary that
/// applications such as Illustrator use for their private editing data.
/// Viewers ignore it, so removing it does not change how the document looks.
fn strip_private_data(object: &mut Object) {
    match object {
        Object::Dictionary(dict) => {
            dict.remove(b"PieceInfo");
            for (_, value) in dict.iter_mut() {
                strip_private_data(value);
            }
        }
        Object::Array(items) => {
            for item in items {
                strip_private_data(item);
            }
        }
        Object::Stream(stream) => {
            stream.dict.remove(b"PieceInfo");
            for (_, value) in stream.dict.iter_mut() {
                strip_private_data(value);
            }
        }
        _ => {}
    }
}

// Handler for Split PDF
//
// The frontend sends an optional `pages` field such as "1-3,5" (1-based, in
// original document order). Only those pages are kept, so the result is a PDF
// containing exactly the selected pages.
async fn split_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;
    let mut pages_spec = String::new();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            // Surfaces the right status (413 for an oversized upload) instead
            // of silently treating it as a missing file field.
            Err(e) => return e.into_response(),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if let Ok(bytes) = field.bytes().await {
                file_data = Some(bytes.to_vec());
            }
        } else if name == "pages" {
            if let Ok(text) = field.text().await {
                pages_spec = text;
            }
        }
    }

    let data = match file_data {
        Some(d) => d,
        None => return bad_request("Missing 'file' field"),
    };

    let result = tokio::task::spawn_blocking(move || {
        let mut doc = load_document(&data)?;
        let page_count = doc.get_pages().len() as u32;

        let selection = if pages_spec.trim().is_empty() {
            (1..=page_count).collect::<Vec<u32>>()
        } else {
            parse_page_selection(&pages_spec, page_count)
        };

        if selection.is_empty() {
            return Err("No valid pages were selected".to_string());
        }

        extract_pages(&mut doc, &selection)?;

        let mut buffer = Vec::new();
        doc.save_to(&mut buffer).map_err(|e| e.to_string())?;
        Ok::<Vec<u8>, String>(buffer)
    })
    .await;

    match result {
        Ok(Ok(bytes)) => send_bytes_response(bytes, "split.pdf"),
        Ok(Err(e)) => {
            error!("Split failed: {}", e);
            bad_request(e)
        }
        Err(e) => {
            error!("Split task failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// Handler for Merge PDFs
async fn merge_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut files_data: Vec<Vec<u8>> = Vec::new();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            // Surfaces the right status (413 for an oversized upload) instead
            // of silently treating it as a missing file field.
            Err(e) => return e.into_response(),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "files" {
            if let Ok(bytes) = field.bytes().await {
                files_data.push(bytes.to_vec());
            }
        }
    }

    if files_data.is_empty() {
        return bad_request("No PDF files were provided");
    }

    let result = tokio::task::spawn_blocking(move || merge_documents(&files_data)).await;

    match result {
        Ok(Ok(bytes)) => send_bytes_response(bytes, "merged.pdf"),
        Ok(Err(e)) => {
            error!("Merge failed: {}", e);
            bad_request(e)
        }
        Err(e) => {
            error!("Merge task failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Merges every document in `files` (PDF bytes) into a single PDF, preserving
/// the page order: all pages of file 1, then all pages of file 2, and so on.
fn merge_documents(files: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let mut documents: Vec<Document> = files
        .iter()
        .map(|data| load_document(data))
        .collect::<Result<_, _>>()?;

    if documents.is_empty() {
        return Err("no input documents".to_string());
    }

    let mut merged = documents.remove(0);
    let base = flatten_page_tree(&mut merged)?;
    let mut all_pages = base.pages;
    // Object numbers must stay unique across documents: shift every incoming
    // object (and every reference to it) above the current maximum.
    let mut next_number = merged.objects.keys().map(|id| id.0).max().unwrap_or(0);
    merged.max_id = merged.max_id.max(next_number);

    for mut source in documents {
        let tree = flatten_page_tree(&mut source)?;
        let offset = next_number;
        let source_max = source.objects.keys().map(|id| id.0).max().unwrap_or(0);
        next_number = next_number.saturating_add(source_max);

        let mut copied = Vec::with_capacity(source.objects.len());
        for (id, mut object) in source.objects {
            remap_references(&mut object, offset);
            copied.push(((id.0.saturating_add(offset), id.1), object));
        }
        merged.objects.extend(copied);

        for page_id in tree.pages {
            all_pages.push((page_id.0.saturating_add(offset), page_id.1));
        }
    }

    merged.max_id = merged.max_id.max(next_number);
    // Points every page at the surviving page-tree root and rewrites /Kids.
    apply_page_order(&mut merged, base.root, &all_pages)?;
    // Drops the source catalogs/page-tree roots that are no longer referenced.
    merged.prune_objects();

    let mut buffer = Vec::new();
    merged.save_to(&mut buffer).map_err(|e| e.to_string())?;
    Ok(buffer)
}

/// Adds `offset` to every indirect reference inside `object`.
fn remap_references(object: &mut Object, offset: u32) {
    match object {
        Object::Reference(id) => {
            id.0 = id.0.saturating_add(offset);
        }
        Object::Array(items) => {
            for item in items {
                remap_references(item, offset);
            }
        }
        Object::Dictionary(dict) => {
            for (_, value) in dict.iter_mut() {
                remap_references(value, offset);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                remap_references(value, offset);
            }
        }
        _ => {}
    }
}

// Handler for Rotate PDF
async fn rotate_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;
    let mut angle_val: Option<i64> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            // Surfaces the right status (413 for an oversized upload) instead
            // of silently treating it as a missing file field.
            Err(e) => return e.into_response(),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if let Ok(bytes) = field.bytes().await {
                file_data = Some(bytes.to_vec());
            }
        } else if name == "angle" {
            if let Ok(text) = field.text().await {
                match text.trim().parse::<i64>() {
                    Ok(parsed) => angle_val = Some(parsed),
                    Err(_) => return bad_request("Rotation angle must be an integer"),
                }
            }
        }
    }

    let data = match file_data {
        Some(d) => d,
        None => return bad_request("Missing 'file' field"),
    };
    let angle = angle_val.unwrap_or(90);

    let result = tokio::task::spawn_blocking(move || {
        let mut doc = load_document(&data)?;
        rotate_pages(&mut doc, angle)?;

        let mut buffer = Vec::new();
        doc.save_to(&mut buffer).map_err(|e| e.to_string())?;
        Ok::<Vec<u8>, String>(buffer)
    })
    .await;

    match result {
        Ok(Ok(bytes)) => send_bytes_response(bytes, "rotated.pdf"),
        Ok(Err(e)) => {
            error!("Rotation failed: {}", e);
            bad_request(e)
        }
        Err(e) => {
            error!("Rotate task failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Rotates every page by `angle` degrees. Only multiples of 90 are valid, and
/// inherited `/Rotate` values from ancestor `/Pages` nodes are taken into
/// account (setting a page value overrides the inherited one).
fn rotate_pages(doc: &mut Document, angle: i64) -> Result<(), String> {
    let normalized = angle.rem_euclid(360);
    if normalized % 90 != 0 {
        return Err("Rotation angle must be a multiple of 90".to_string());
    }

    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for page_id in pages {
        let current = effective_rotation(doc, page_id);
        let new_rotation = (current + normalized).rem_euclid(360);

        let page = doc
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|e| format!("invalid page {:?}: {}", page_id, e))?;
        page.set("Rotate", Object::Integer(new_rotation));
    }

    Ok(())
}

/// Resolves a page's effective `/Rotate`, following the `/Parent` chain when
/// the value is inherited from a `/Pages` node.
fn effective_rotation(doc: &Document, page_id: ObjectId) -> i64 {
    let mut node_id = page_id;

    for _ in 0..256 {
        let node = match doc.get_dictionary(node_id) {
            Ok(node) => node,
            Err(_) => break,
        };

        if let Ok(rotate) = node.get(b"Rotate").and_then(Object::as_i64) {
            return rotate;
        }

        match node.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => node_id = parent,
            Err(_) => break,
        }
    }

    0
}

// Handler for Organize PDF
async fn organize_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;
    let mut order_str: String = String::new();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            // Surfaces the right status (413 for an oversized upload) instead
            // of silently treating it as a missing file field.
            Err(e) => return e.into_response(),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if let Ok(bytes) = field.bytes().await {
                file_data = Some(bytes.to_vec());
            }
        } else if name == "order" {
            if let Ok(text) = field.text().await {
                order_str = text;
            }
        }
    }

    let data = match file_data {
        Some(d) => d,
        None => return bad_request("Missing 'file' field"),
    };

    let result = tokio::task::spawn_blocking(move || {
        let mut doc = load_document(&data)?;

        let requested_order: Vec<u32> = order_str
            .split(',')
            .filter_map(|part| part.trim().parse::<u32>().ok())
            .collect();

        if !requested_order.is_empty() {
            reorder_pages(&mut doc, &requested_order)?;
            // Removes the intermediate page-tree nodes that flattening replaced.
            doc.prune_objects();
        }

        let mut buffer = Vec::new();
        doc.save_to(&mut buffer).map_err(|e| e.to_string())?;
        Ok::<Vec<u8>, String>(buffer)
    })
    .await;

    match result {
        Ok(Ok(bytes)) => send_bytes_response(bytes, "organized.pdf"),
        Ok(Err(e)) => {
            error!("Organize failed: {}", e);
            bad_request(e)
        }
        Err(e) => {
            error!("Organize task failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Page attributes that are inherited from ancestor `/Pages` nodes (PDF 32000-1:2008, 7.7.3.4).
/// They must be copied down to each page when the page tree is flattened.
const INHERITABLE_PAGE_KEYS: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// A page tree flattened to a single `/Pages` node.
struct PageTree {
    root: ObjectId,
    /// Page object ids in reading order.
    pages: Vec<ObjectId>,
}

/// Flattens the page tree of `doc` into a single root node.
///
/// Flattening means copying the attributes each page inherited from its
/// ancestor `/Pages` nodes down into the page itself, repointing every
/// `/Parent` at the root, and rewriting `/Kids` in reading order. After this
/// call the document's pages can be reordered, kept or dropped safely.
fn flatten_page_tree(doc: &mut Document) -> Result<PageTree, String> {
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err("document has no pages".to_string());
    }

    let root_id = doc
        .catalog()
        .and_then(|catalog| catalog.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| format!("invalid page tree: {}", e))?;

    // Remember what each page used to inherit from its ancestors.
    let mut inherited: HashMap<ObjectId, Dictionary> = HashMap::new();
    let mut ancestors: Vec<Dictionary> = Vec::new();
    collect_inheritable_attributes(doc, root_id, &mut ancestors, &mut inherited);

    let ordered: Vec<ObjectId> = pages.values().copied().collect();
    for page_id in &ordered {
        let page = doc
            .get_object_mut(*page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|e| format!("invalid page {:?}: {}", page_id, e))?;

        if let Some(attrs) = inherited.get(page_id) {
            for (key, value) in attrs.iter() {
                if !page.has(key) {
                    page.set(key.clone(), value.clone());
                }
            }
        }
    }

    apply_page_order(doc, root_id, &ordered)?;
    Ok(PageTree {
        root: root_id,
        pages: ordered,
    })
}

/// Repoints every page's `/Parent` at `root_id` and rewrites the root `/Kids`
/// and `/Count` to match `order` exactly.
fn apply_page_order(
    doc: &mut Document,
    root_id: ObjectId,
    order: &[ObjectId],
) -> Result<(), String> {
    for page_id in order {
        let page = doc
            .get_object_mut(*page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|e| format!("invalid page {:?}: {}", page_id, e))?;
        page.set(b"Parent", Object::Reference(root_id));
    }

    let kids: Vec<Object> = order.iter().map(|id| Object::Reference(*id)).collect();
    let root = doc
        .get_object_mut(root_id)
        .and_then(Object::as_dict_mut)
        .map_err(|e| format!("invalid root page tree: {}", e))?;
    root.set(b"Type", Object::Name(b"Pages".to_vec()));
    root.set(b"Kids", Object::Array(kids));
    root.set(b"Count", Object::Integer(order.len() as i64));

    Ok(())
}

/// Parses selections like `"1-3,5"` / `"7,2"` into 1-based page numbers,
/// discarding out-of-range values and duplicates while preserving order.
/// Ranges are clamped before iterating, so `"1-4294967295"` cannot allocate
/// billions of entries.
fn parse_page_selection(spec: &str, page_count: u32) -> Vec<u32> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();

    if page_count == 0 {
        return selected;
    }

    for token in spec.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        if let Some((start, end)) = token.split_once('-') {
            let (Ok(start), Ok(end)) = (start.trim().parse::<u32>(), end.trim().parse::<u32>())
            else {
                continue;
            };

            let (low, high, descending) = if start <= end {
                (start, end, false)
            } else {
                (end, start, true)
            };
            let low = low.max(1);
            let high = high.min(page_count);
            if low > high {
                continue;
            }

            if descending {
                for number in (low..=high).rev() {
                    if seen.insert(number) {
                        selected.push(number);
                    }
                }
            } else {
                for number in low..=high {
                    if seen.insert(number) {
                        selected.push(number);
                    }
                }
            }
        } else if let Ok(number) = token.parse::<u32>() {
            if (1..=page_count).contains(&number) && seen.insert(number) {
                selected.push(number);
            }
        }
    }

    selected
}

/// Keeps only the selected 1-based pages, in the given order, and drops the
/// rest of the document (unselected pages become unreachable and are pruned).
fn extract_pages(doc: &mut Document, selection: &[u32]) -> Result<(), String> {
    let tree = flatten_page_tree(doc)?;
    let selected: Vec<ObjectId> = selection
        .iter()
        .filter_map(|number| {
            number
                .checked_sub(1)
                .and_then(|index| tree.pages.get(index as usize))
                .copied()
        })
        .collect();

    if selected.is_empty() {
        return Err("No valid pages were selected".to_string());
    }

    apply_page_order(doc, tree.root, &selected)?;
    doc.prune_objects();
    Ok(())
}

/// Reorders the pages of `doc` according to `order`.
///
/// `order` holds 1-based page numbers in the document's *original* order.
/// Out-of-range indices and duplicates are ignored, and pages that are not
/// mentioned are appended at the end so no page is ever lost.
fn reorder_pages(doc: &mut Document, order: &[u32]) -> Result<(), String> {
    let tree = flatten_page_tree(doc)?;

    let mut new_order: Vec<ObjectId> = Vec::with_capacity(tree.pages.len());
    let mut seen: HashSet<ObjectId> = HashSet::new();
    for page_number in order {
        if *page_number == 0 {
            continue;
        }
        if let Some(&id) = tree.pages.get((*page_number - 1) as usize) {
            if seen.insert(id) {
                new_order.push(id);
            }
        }
    }
    for id in &tree.pages {
        if seen.insert(*id) {
            new_order.push(*id);
        }
    }

    apply_page_order(doc, tree.root, &new_order)
}

fn collect_inheritable_attributes(
    doc: &Document,
    node_id: ObjectId,
    ancestors: &mut Vec<Dictionary>,
    inherited: &mut HashMap<ObjectId, Dictionary>,
) {
    // Guard against cyclic or absurdly deep page trees in malformed files.
    if ancestors.len() >= 256 {
        return;
    }

    let node = match doc.get_dictionary(node_id) {
        Ok(node) => node,
        Err(_) => return,
    };

    let mut attributes = Dictionary::new();
    for key in INHERITABLE_PAGE_KEYS {
        if let Ok(value) = node.get(key) {
            attributes.set(key, value.clone());
        }
    }
    ancestors.push(attributes);

    match node.get(b"Kids").and_then(Object::as_array) {
        Ok(kids) => {
            let children: Vec<ObjectId> = kids
                .iter()
                .filter_map(|kid| kid.as_reference().ok())
                .collect();
            for child in children {
                collect_inheritable_attributes(doc, child, ancestors, inherited);
            }
        }
        Err(_) => {
            // Leaf node: a page. Nearest ancestor wins, so merge root-to-leaf.
            let mut merged = Dictionary::new();
            for level in ancestors.iter() {
                for (key, value) in level.iter() {
                    merged.set(key.clone(), value.clone());
                }
            }
            inherited.insert(node_id, merged);
        }
    }

    ancestors.pop();
}

// Handler for Pages Info (returns page count for the interactive views)
async fn pages_info_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            // Surfaces the right status (413 for an oversized upload) instead
            // of silently treating it as a missing file field.
            Err(e) => return e.into_response(),
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if let Ok(bytes) = field.bytes().await {
                file_data = Some(bytes.to_vec());
                break;
            }
        }
    }

    let data = match file_data {
        Some(d) => d,
        None => return bad_request("Missing 'file' field"),
    };

    let result = tokio::task::spawn_blocking(move || {
        let doc = load_document(&data)?;
        Ok::<usize, String>(doc.get_pages().len())
    })
    .await;

    match result {
        Ok(Ok(page_count)) => Json(PageInfoResponse { page_count }).into_response(),
        Ok(Err(e)) => {
            error!("Pages info failed: {}", e);
            bad_request(e)
        }
        Err(e) => {
            error!("Pages info task failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn bad_request(message: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, message.into()).into_response()
}

fn send_bytes_response(bytes: Vec<u8>, filename: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};
    use tempfile::NamedTempFile;

    /// Builds a flat PDF where every page contains a distinct text label.
    fn make_pdf(labels: &[&str]) -> Document {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });

        let mut kids = Vec::new();
        for label in labels {
            let content = format!("BT /F1 24 Tf 72 720 Td ({}) Tj ET", label);
            let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            });
            kids.push(Object::Reference(page_id));
        }

        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => labels.len() as i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc
    }

    /// Returns the text painted by the page at the given 1-based position.
    fn page_text(doc: &Document, page_number: u32) -> String {
        let page_id = doc.get_pages()[&page_number];
        let mut text = String::new();
        for content_id in doc.get_page_contents(page_id) {
            if let Ok(stream) = doc.get_object(content_id).and_then(Object::as_stream) {
                text.push_str(&String::from_utf8_lossy(&stream.content));
            }
        }
        text
    }

    fn pdf_bytes(mut doc: Document) -> Vec<u8> {
        let mut buffer = Vec::new();
        doc.save_to(&mut buffer).unwrap();
        buffer
    }

    #[test]
    fn parses_page_selection_in_order_and_ignores_invalid_values() {
        assert_eq!(parse_page_selection("1-3,5", 6), vec![1, 2, 3, 5]);
        assert_eq!(parse_page_selection(" 4, 2 ,2, 9, x ", 4), vec![4, 2]);
        assert_eq!(parse_page_selection("5-3", 6), vec![5, 4, 3]);
        assert!(parse_page_selection("", 3).is_empty());
        assert!(parse_page_selection("0,7,-2", 3).is_empty());
    }

    #[test]
    fn split_keeps_only_selected_pages_in_requested_order() {
        let mut doc = make_pdf(&["PAGE ONE", "PAGE TWO", "PAGE THREE", "PAGE FOUR"]);
        let selection = parse_page_selection("3,1", 4);
        extract_pages(&mut doc, &selection).unwrap();

        assert_eq!(doc.get_pages().len(), 2);
        assert!(page_text(&doc, 1).contains("PAGE THREE"));
        assert!(page_text(&doc, 2).contains("PAGE ONE"));

        // The kept pages must survive a save/load round-trip.
        let file = NamedTempFile::new().unwrap();
        doc.save(file.path()).unwrap();
        let reloaded = Document::load(file.path()).unwrap();
        assert_eq!(reloaded.get_pages().len(), 2);
        assert!(page_text(&reloaded, 1).contains("PAGE THREE"));
        assert!(page_text(&reloaded, 2).contains("PAGE ONE"));
    }

    #[test]
    fn compression_strips_private_data_and_never_grows() {
        let mut doc = make_pdf(&["PAGE ONE"]);
        let private_id = doc.add_object(Stream::new(
            dictionary! { "Subtype" => "XML" },
            vec![0xAB; 4096],
        ));
        let page_id = doc.get_pages()[&1];
        doc.get_object_mut(page_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set(
                "PieceInfo",
                dictionary! { "Illustrator" => dictionary! { "Private" => private_id } },
            );

        let input = pdf_bytes(doc);
        let output = compress_pdf(&input).unwrap();

        assert!(output.len() < input.len(), "expected a smaller PDF");
        let reloaded = Document::load_mem(&output).unwrap();
        let page = reloaded.get_dictionary(reloaded.get_pages()[&1]).unwrap();
        assert!(!page.has(b"PieceInfo"), "PieceInfo should be removed");
        assert!(
            reloaded.get_object(private_id).is_err(),
            "the unreachable private stream should be pruned"
        );
    }

    #[test]
    fn rejects_password_protected_pdfs() {
        let mut doc = make_pdf(&["PAGE ONE"]);
        let encrypt_id = doc.add_object(dictionary! {
            "Filter" => "Standard",
            "V" => 2,
            "R" => 3,
        });
        doc.trailer.set("Encrypt", Object::Reference(encrypt_id));

        let bytes = pdf_bytes(doc);
        let error = match load_document(&bytes) {
            Ok(_) => panic!("encrypted PDFs must be rejected"),
            Err(error) => error,
        };
        assert!(
            error.contains("Password-protected"),
            "unexpected error: {}",
            error
        );
    }

    #[test]
    fn merge_appends_documents_in_order_and_remaps_objects() {
        let first = pdf_bytes(make_pdf(&["ONE", "TWO"]));
        let second = pdf_bytes(make_pdf(&["THREE", "FOUR", "FIVE"]));

        let bytes = merge_documents(&[first, second]).unwrap();
        let merged = Document::load_mem(&bytes).unwrap();

        assert_eq!(merged.get_pages().len(), 5);
        for (index, expected) in ["ONE", "TWO", "THREE", "FOUR", "FIVE"].iter().enumerate() {
            assert!(
                page_text(&merged, (index + 1) as u32).contains(expected),
                "page {} should contain {}",
                index + 1,
                expected
            );
        }

        // Every page must belong to the merged page tree, not to a source one.
        let root = merged
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        for (_, page_id) in merged.get_pages() {
            let page = merged.get_dictionary(page_id).unwrap();
            assert_eq!(
                page.get(b"Parent").unwrap().as_reference().unwrap(),
                root,
                "page {:?} still points at a stale page tree",
                page_id
            );
        }
    }

    #[test]
    fn reorders_pages_using_one_based_indices() {
        let mut doc = make_pdf(&["PAGE ONE", "PAGE TWO", "PAGE THREE"]);
        reorder_pages(&mut doc, &[3, 1, 2]).unwrap();

        assert!(page_text(&doc, 1).contains("PAGE THREE"));
        assert!(page_text(&doc, 2).contains("PAGE ONE"));
        assert!(page_text(&doc, 3).contains("PAGE TWO"));

        // Make sure the reordered document survives a save/load round-trip.
        let file = NamedTempFile::new().unwrap();
        doc.save(file.path()).unwrap();
        let reloaded = Document::load(file.path()).unwrap();
        assert_eq!(reloaded.get_pages().len(), 3);
        assert!(page_text(&reloaded, 1).contains("PAGE THREE"));
        assert!(page_text(&reloaded, 2).contains("PAGE ONE"));
        assert!(page_text(&reloaded, 3).contains("PAGE TWO"));
    }

    #[test]
    fn ignores_invalid_indices_and_appends_unlisted_pages() {
        let mut doc = make_pdf(&["PAGE ONE", "PAGE TWO", "PAGE THREE"]);
        // Only page 2 is requested; the rest must be kept, in original order.
        reorder_pages(&mut doc, &[2, 99]).unwrap();

        assert_eq!(doc.get_pages().len(), 3);
        assert!(page_text(&doc, 1).contains("PAGE TWO"));
        assert!(page_text(&doc, 2).contains("PAGE ONE"));
        assert!(page_text(&doc, 3).contains("PAGE THREE"));
    }

    #[test]
    fn pushes_down_attributes_inherited_from_parent_pages_nodes() {
        let mut doc = Document::with_version("1.5");
        let root_id = doc.new_object_id();
        let inner_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });

        let contents_one = doc.add_object(Stream::new(dictionary! {}, b"ONE".to_vec()));
        let contents_two = doc.add_object(Stream::new(dictionary! {}, b"TWO".to_vec()));

        let page_one = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => inner_id,
            "Contents" => contents_one,
        });
        let page_two = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => inner_id,
            "Contents" => contents_two,
        });

        // Intermediate node inherits MediaBox/Resources from the root and adds Rotate.
        doc.objects.insert(
            inner_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Parent" => root_id,
                "Kids" => vec![Object::Reference(page_one), Object::Reference(page_two)],
                "Count" => 2,
                "Rotate" => 90,
            }),
        );
        doc.objects.insert(
            root_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(inner_id)],
                "Count" => 2,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => root_id,
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        reorder_pages(&mut doc, &[2, 1]).unwrap();

        assert!(page_text(&doc, 1).contains("TWO"));
        assert!(page_text(&doc, 2).contains("ONE"));

        // Every page must now carry the attributes it used to inherit.
        for page_number in 1..=2 {
            let page_id = doc.get_pages()[&page_number];
            let page = doc.get_dictionary(page_id).unwrap();
            assert!(page.has(b"MediaBox"), "page {} lost MediaBox", page_number);
            assert!(
                page.has(b"Resources"),
                "page {} lost Resources",
                page_number
            );
            assert_eq!(
                page.get(b"Rotate").and_then(Object::as_i64).unwrap(),
                90,
                "page {} lost inherited Rotate",
                page_number
            );
            assert_eq!(
                page.get(b"Parent").and_then(Object::as_reference).unwrap(),
                root_id
            );
        }

        let file = NamedTempFile::new().unwrap();
        doc.save(file.path()).unwrap();
        let reloaded = Document::load(file.path()).unwrap();
        assert!(page_text(&reloaded, 1).contains("TWO"));
        assert!(page_text(&reloaded, 2).contains("ONE"));
    }

    #[test]
    fn page_selection_clamps_huge_and_inverted_ranges() {
        assert_eq!(
            parse_page_selection("1-4294967295", 4),
            vec![1, 2, 3, 4],
            "huge ranges must be clamped instead of allocated"
        );
        assert_eq!(
            parse_page_selection("4294967295-1", 4),
            vec![4, 3, 2, 1],
            "inverted ranges keep their descending order"
        );
        assert_eq!(parse_page_selection("0-2", 4), vec![1, 2]);
        assert_eq!(parse_page_selection("3-9", 4), vec![3, 4]);
    }

    #[test]
    fn split_ignores_page_zero_instead_of_panicking() {
        let mut doc = make_pdf(&["PAGE ONE", "PAGE TWO"]);
        extract_pages(&mut doc, &[0, 2]).unwrap();

        assert_eq!(doc.get_pages().len(), 1);
        assert!(page_text(&doc, 1).contains("PAGE TWO"));
    }

    #[test]
    fn rotation_respects_inherited_rotate_and_validates_angle() {
        let mut doc = make_pdf(&["PAGE ONE"]);
        let root_id = doc
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_object_mut(root_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Rotate", Object::Integer(90));

        // The page inherits /Rotate 90, so +90 must result in 180 (not 90).
        rotate_pages(&mut doc, 90).unwrap();
        let page_id = doc.get_pages()[&1];
        let page = doc.get_dictionary(page_id).unwrap();
        assert_eq!(
            page.get(b"Rotate").and_then(Object::as_i64).unwrap(),
            180,
            "inherited rotation was ignored"
        );

        // Negative angles are normalized modulo 360.
        rotate_pages(&mut doc, -90).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        assert_eq!(page.get(b"Rotate").and_then(Object::as_i64).unwrap(), 90);

        assert!(rotate_pages(&mut doc, 45).is_err());
    }
}
