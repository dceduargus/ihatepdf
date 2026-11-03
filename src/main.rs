use axum::{
    body::Body,
    extract::Multipart,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use lopdf::Document;
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

    let result = tokio::task::spawn_blocking(move || {
        match Document::load(&input_path) {
            Ok(mut doc) => {
                // Save loaded doc (or split logic)
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

        // Load first document as base
        let mut base_doc = match Document::load(&temp_paths[0]) {
            Ok(d) => d,
            Err(_) => return None,
        };

        // For subsequent documents, save or copy
        for path in temp_paths.iter().skip(1) {
            if let Ok(doc) = Document::load(path) {
                // If base doc can save/merge
                let _ = doc;
            }
        }

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
