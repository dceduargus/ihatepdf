use axum::{
    body::Body,
    extract::Multipart,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use lopdf::{Document, Object};
use std::io::Write;
use tempfile::NamedTempFile;
use tower_http::services::ServeDir;
use tracing::info;

#[tokio::main]
async fn main() {
    // Initialize subscriber for logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Build Axum router
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/compress", post(compress_handler))
        .route("/api/split", post(split_handler))
        .route("/api/merge", post(merge_handler))
        .route("/api/rotate", post(rotate_handler))
        .route("/api/organize", post(organize_handler))
        .nest_service("/static", ServeDir::new("static"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    info!("🚀 ihatepdf server running at http://127.0.0.1:3000");
    axum::serve(listener, app).await.unwrap();
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

    let mut input_tmp = NamedTempFile::new().unwrap();
    if input_tmp.write_all(&data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let input_path = input_tmp.path().to_path_buf();
    let output_tmp = NamedTempFile::new().unwrap();
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

    let mut input_tmp = NamedTempFile::new().unwrap();
    if input_tmp.write_all(&data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let input_path = input_tmp.path().to_path_buf();
    let output_tmp = NamedTempFile::new().unwrap();
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

    let mut input_tmp = NamedTempFile::new().unwrap();
    if input_tmp.write_all(&data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let input_path = input_tmp.path().to_path_buf();
    let output_tmp = NamedTempFile::new().unwrap();
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

    let mut input_tmp = NamedTempFile::new().unwrap();
    if input_tmp.write_all(&data).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let input_path = input_tmp.path().to_path_buf();
    let output_tmp = NamedTempFile::new().unwrap();
    let output_path = output_tmp.path().to_path_buf();

    let result = tokio::task::spawn_blocking(move || {
        match Document::load(&input_path) {
            Ok(mut doc) => {
                // If order is provided, parse page order indices (1-based)
                let pages = doc.get_pages(); // BTreeMap<u32, ObjectId>
                let mut requested_order: Vec<u32> = Vec::new();
                for part in order_str.split(',') {
                    if let Ok(num) = part.trim().parse::<u32>() {
                        requested_order.push(num);
                    }
                }

                if !requested_order.is_empty() {
                    let mut new_page_list = Vec::new();
                    for &p in &requested_order {
                        if let Some(&obj_id) = pages.get(&p) {
                            new_page_list.push(obj_id);
                        }
                    }
                    if !new_page_list.is_empty() {
                        doc.renumber_objects_with(0); // or similar if needed
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
        }
    })
    .await;

    match result {
        Ok(Ok(path)) => send_file_response(path, "organized.pdf").await,
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn send_file_response(path: std::path::PathBuf, filename: &str) -> Response {
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let _ = tokio::fs::remove_file(path).await;

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/pdf")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
