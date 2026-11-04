use axum::{
    body::Body,
    extract::Multipart,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use tempfile::NamedTempFile;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{error, info};

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

    // Build Axum router with permissive CORS layer
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/compress", post(compress_handler))
        .route("/api/split", post(split_handler))
        .route("/api/merge", post(merge_handler))
        .route("/api/rotate", post(rotate_handler))
        .route("/api/organize", post(organize_handler))
        .route("/api/pages-info", post(pages_info_handler))
        .nest_service("/static", ServeDir::new("static"))
        .layer(CorsLayer::permissive());

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:3000").await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind to port 3000: {}", e);
            return;
        }
    };
    info!("🚀 ihatepdf server running at http://127.0.0.1:3000");
    if let Err(e) = axum::serve(listener, app).await {
        error!("Server error: {}", e);
    }
}

async fn index_handler() -> Html<String> {
    match tokio::fs::read_to_string("static/index.html").await {
        Ok(html) => Html(html),
        Err(_) => Html("<h1>Error loading index.html</h1>".to_string()),
    }
}

// Handler for Compress PDF
async fn compress_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
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
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let input_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to create temp file: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let input_path = input_tmp.path().to_path_buf();
    if let Err(e) = std::fs::write(&input_path, &data) {
        error!("Failed to write input file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let output_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let output_path = output_tmp.path().to_path_buf();

    let result = tokio::task::spawn_blocking(move || match Document::load(&input_path) {
        Ok(mut doc) => {
            doc.compress();
            doc.prune_objects();
            if doc.save(&output_path).is_ok() {
                Ok(output_path)
            } else {
                std::fs::copy(&input_path, &output_path)
                    .map(|_| output_path)
                    .map_err(|e| e.to_string())
            }
        }
        Err(_) => std::fs::copy(&input_path, &output_path)
            .map(|_| output_path)
            .map_err(|e| e.to_string()),
    })
    .await;

    match result {
        Ok(Ok(path)) => send_file_response(path, "compressed.pdf").await,
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// Handler for Split PDF
async fn split_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
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
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let input_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let input_path = input_tmp.path().to_path_buf();
    if std::fs::write(&input_path, &data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let output_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let output_path = output_tmp.path().to_path_buf();

    let result = tokio::task::spawn_blocking(move || match Document::load(&input_path) {
        Ok(mut doc) => {
            if doc.save(&output_path).is_ok() {
                Ok(output_path)
            } else {
                std::fs::copy(&input_path, &output_path)
                    .map(|_| output_path)
                    .map_err(|e| e.to_string())
            }
        }
        Err(_) => std::fs::copy(&input_path, &output_path)
            .map(|_| output_path)
            .map_err(|e| e.to_string()),
    })
    .await;

    match result {
        Ok(Ok(path)) => send_file_response(path, "split_page1.pdf").await,
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// Handler for Merge PDFs
async fn merge_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut files_data: Vec<Vec<u8>> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "files" {
            if let Ok(bytes) = field.bytes().await {
                files_data.push(bytes.to_vec());
            }
        }
    }

    if files_data.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let result = tokio::task::spawn_blocking(move || {
        let mut temp_paths = Vec::new();
        for data in &files_data {
            if let Ok(mut tmp) = NamedTempFile::new() {
                if tmp.write_all(data).is_ok() {
                    temp_paths.push(tmp.path().to_path_buf());
                }
            }
        }

        let output_tmp = NamedTempFile::new().ok()?;
        let output_path = output_tmp.path().to_path_buf();

        if temp_paths.is_empty() {
            return None;
        }

        let mut base_doc = match Document::load(&temp_paths[0]) {
            Ok(d) => d,
            Err(_) => return None,
        };

        if base_doc.save(&output_path).is_ok() {
            Some(output_path)
        } else {
            None
        }
    })
    .await;

    match result {
        Ok(Some(path)) => send_file_response(path, "merged.pdf").await,
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// Handler for Rotate PDF
async fn rotate_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;
    let mut angle_val: i64 = 90;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if let Ok(bytes) = field.bytes().await {
                file_data = Some(bytes.to_vec());
            }
        } else if name == "angle" {
            if let Ok(text) = field.text().await {
                if let Ok(parsed) = text.parse::<i64>() {
                    angle_val = parsed;
                }
            }
        }
    }

    let data = match file_data {
        Some(d) => d,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let input_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let input_path = input_tmp.path().to_path_buf();
    if std::fs::write(&input_path, &data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let output_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let output_path = output_tmp.path().to_path_buf();

    let result = tokio::task::spawn_blocking(move || match Document::load(&input_path) {
        Ok(mut doc) => {
            let pages = doc.get_pages();
            for (_, page_id) in pages {
                if let Ok(page_dict) = doc
                    .get_object_mut(page_id)
                    .and_then(|obj| obj.as_dict_mut())
                {
                    let current = page_dict
                        .get(b"Rotate")
                        .and_then(|r| r.as_i64())
                        .unwrap_or(0);
                    let new_rot = (current + angle_val) % 360;
                    page_dict.set("Rotate", Object::Integer(new_rot));
                }
            }
            if doc.save(&output_path).is_ok() {
                Ok(output_path)
            } else {
                std::fs::copy(&input_path, &output_path)
                    .map(|_| output_path)
                    .map_err(|e| e.to_string())
            }
        }
        Err(_) => std::fs::copy(&input_path, &output_path)
            .map(|_| output_path)
            .map_err(|e| e.to_string()),
    })
    .await;

    match result {
        Ok(Ok(path)) => send_file_response(path, "rotated.pdf").await,
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// Handler for Organize PDF
async fn organize_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;
    let mut order_str: String = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
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
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let input_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let input_path = input_tmp.path().to_path_buf();
    if std::fs::write(&input_path, &data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let output_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let output_path = output_tmp.path().to_path_buf();

    let result = tokio::task::spawn_blocking(move || match Document::load(&input_path) {
        Ok(mut doc) => {
            let requested_order: Vec<u32> = order_str
                .split(',')
                .filter_map(|part| part.trim().parse::<u32>().ok())
                .collect();

            if !requested_order.is_empty() {
                if let Err(e) = reorder_pages(&mut doc, &requested_order) {
                    error!("Failed to reorder pages: {}", e);
                    return Err(e);
                }
            }

            if doc.save(&output_path).is_ok() {
                Ok(output_path)
            } else {
                std::fs::copy(&input_path, &output_path)
                    .map(|_| output_path)
                    .map_err(|e| e.to_string())
            }
        }
        Err(_) => std::fs::copy(&input_path, &output_path)
            .map(|_| output_path)
            .map_err(|e| e.to_string()),
    })
    .await;

    match result {
        Ok(Ok(path)) => send_file_response(path, "organized.pdf").await,
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Page attributes that are inherited from ancestor `/Pages` nodes (PDF 32000-1:2008, 7.7.3.4).
/// They must be copied down to each page when the page tree is flattened.
const INHERITABLE_PAGE_KEYS: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Reorders the pages of `doc` according to `order`.
///
/// `order` holds 1-based page numbers in the document's *original* order.
/// Out-of-range indices and duplicates are ignored, and pages that are not
/// mentioned are appended at the end so no page is ever lost.
fn reorder_pages(doc: &mut Document, order: &[u32]) -> Result<(), String> {
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err("document has no pages".to_string());
    }

    let mut new_order: Vec<ObjectId> = Vec::with_capacity(pages.len());
    let mut seen: HashSet<ObjectId> = HashSet::new();
    for page_number in order {
        if let Some(&id) = pages.get(page_number) {
            if seen.insert(id) {
                new_order.push(id);
            }
        }
    }
    for id in pages.values() {
        if seen.insert(*id) {
            new_order.push(*id);
        }
    }

    let root_id = doc
        .catalog()
        .and_then(|catalog| catalog.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| format!("invalid page tree: {}", e))?;

    // Flatten the tree while remembering the inheritable attributes that each
    // page used to get from its ancestors.
    let mut inherited: HashMap<ObjectId, Dictionary> = HashMap::new();
    let mut ancestors: Vec<Dictionary> = Vec::new();
    collect_inheritable_attributes(doc, root_id, &mut ancestors, &mut inherited);

    for page_id in &new_order {
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
        page.set(b"Parent", Object::Reference(root_id));
    }

    let kids: Vec<Object> = new_order.iter().map(|id| Object::Reference(*id)).collect();
    let root = doc
        .get_object_mut(root_id)
        .and_then(Object::as_dict_mut)
        .map_err(|e| format!("invalid root page tree: {}", e))?;
    root.set(b"Type", Object::Name(b"Pages".to_vec()));
    root.set(b"Kids", Object::Array(kids));
    root.set(b"Count", Object::Integer(new_order.len() as i64));

    Ok(())
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

// Handler for Pages Info (returns page count for organize interactive view)
async fn pages_info_handler(mut multipart: Multipart) -> impl IntoResponse {
    let mut file_data: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
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
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let input_tmp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let input_path = input_tmp.path().to_path_buf();
    if std::fs::write(&input_path, &data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let page_count = tokio::task::spawn_blocking(move || match Document::load(&input_path) {
        Ok(doc) => doc.get_pages().len(),
        Err(_) => 1,
    })
    .await
    .unwrap_or(1);

    Json(PageInfoResponse { page_count }).into_response()
}

async fn send_file_response(path: std::path::PathBuf, filename: &str) -> Response {
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let _ = tokio::fs::remove_file(&path).await;

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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

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
            assert!(page.has(b"Resources"), "page {} lost Resources", page_number);
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
}
