//! Optional static web export for development (`RING_WEB_DIR`): `/x` serves `x`, `x.html` or
//! `x/index.html`, else `404.html`.

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::app::AppState;

fn candidates(root: &Path, path: &str) -> Vec<PathBuf> {
    let mut clean = PathBuf::new();
    for component in Path::new(path.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            _ => return vec![],
        }
    }
    let base = root.join(&clean);
    let mut out = vec![base.clone()];
    if let Some(name) = base.file_name() {
        let mut html = name.to_os_string();
        html.push(".html");
        out.push(base.with_file_name(html));
    }
    out.push(base.join("index.html"));
    out
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wav" => "audio/wav",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub async fn serve(State(app): State<AppState>, request: Request) -> Response {
    let Some(root) = app.config.web_dir.clone() else {
        return crate::error::ApiError::not_found("Route").into_response();
    };
    let path = request.uri().path().to_string();
    let mut found = None;
    for candidate in candidates(&root, &path) {
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|m| m.is_file())
        {
            found = Some((candidate, StatusCode::OK));
            break;
        }
    }
    let (file, status) = match found {
        Some(f) => f,
        None => (root.join("404.html"), StatusCode::NOT_FOUND),
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(content_type(&file)),
            );
            response
        }
        Err(_) => crate::error::ApiError::not_found("Page").into_response(),
    }
}
