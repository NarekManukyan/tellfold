//! The Studio frontend (embedded at build time) and file responses with Starlette's headers.

use std::path::Path;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::error::{ApiError, ApiResult};

include!(concat!(env!("OUT_DIR"), "/web_files.rs"));

const MISSING_FRONTEND: &str =
    "<!doctype html><title>Tellfold Studio</title><p>The Tellfold Studio frontend files are \
missing from this installation (web/index.html in tellfold-server).</p>";

pub fn embedded(path: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(name, _)| *name == path).map(|(_, data)| *data)
}

/// `mimetypes.guess_type` for the files the server sends, with Starlette's `; charset=utf-8` on text types.
pub fn guess_type(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let suffix = lower.rsplit_once('.').map_or("", |(_, s)| s);
    match suffix {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/vnd.microsoft.icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "text/plain; charset=utf-8",
    }
}

fn bytes_response(data: Vec<u8>, method: &Method, content_type: &str) -> Response {
    let length = data.len();
    let body = if method == Method::HEAD { Body::empty() } else { Body::from(data) };
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).expect("a valid content type"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    response
}

/// A file's contents in 64 KiB chunks.
fn file_chunks(
    file: tokio::fs::File,
) -> impl futures::Stream<Item = std::io::Result<axum::body::Bytes>> + Send + 'static {
    use tokio::io::AsyncReadExt;
    futures::stream::unfold(Some(file), |file| async move {
        let mut file = file?;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(axum::body::Bytes::from(buf)), Some(file)))
            }
            // the error ends the body; nothing is read after it
            Err(e) => Some((Err(e), None)),
        }
    })
}

/// `quote(name)` with Python's default safe characters.
fn py_quote(text: &str) -> String {
    const SAFE: &percent_encoding::AsciiSet =
        &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b'/');
    percent_encoding::utf8_percent_encode(text, SAFE).to_string()
}

/// Starlette's `FileResponse`: the file with its type, length, modification time and (with a name) an attachment
/// disposition.
pub async fn file_response(
    path: &Path,
    method: &Method,
    content_type: &str,
    attachment: Option<&str>,
    extra: &[(&'static str, &'static str)],
) -> ApiResult {
    let file = tokio::fs::File::open(path).await.map_err(ApiError::internal)?;
    open_file_response(file, method, content_type, attachment, extra).await
}

/// [`file_response`] for a file already open (one that may have no name on disk), streamed from where it is.
pub async fn open_file_response(
    file: tokio::fs::File,
    method: &Method,
    content_type: &str,
    attachment: Option<&str>,
    extra: &[(&'static str, &'static str)],
) -> ApiResult {
    let meta = file.metadata().await.map_err(ApiError::internal)?;
    let modified = meta.modified().ok();
    // streamed in chunks, and a HEAD request only stats the file
    let body = if method == Method::HEAD { Body::empty() } else { Body::from_stream(file_chunks(file)) };
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).map_err(ApiError::internal)?);
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
    if let Some(name) = attachment {
        let quoted = py_quote(name);
        let disposition = if quoted != name {
            format!("attachment; filename*=utf-8''{quoted}")
        } else {
            format!("attachment; filename=\"{name}\"")
        };
        headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disposition).map_err(ApiError::internal)?);
    }
    if let Some(modified) = modified {
        let date = httpdate::fmt_http_date(modified);
        headers.insert(header::LAST_MODIFIED, HeaderValue::from_str(&date).map_err(ApiError::internal)?);
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    for (name, value) in extra {
        headers.insert(*name, HeaderValue::from_static(value));
    }
    Ok(response)
}

/// A file under `/static/`, or Starlette's `{"detail": "Not Found"}`.
pub fn static_response(path: &str, method: &Method) -> Response {
    match embedded(&format!("static/{path}")).filter(|_| !path.is_empty()) {
        Some(data) => {
            let mut response = bytes_response(data.to_vec(), method, guess_type(path));
            response.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            response
        }
        None => ApiError::new(404, "Not Found").into_response(),
    }
}

/// The single-page app (or a 503 page when the frontend is not in this build).
pub fn index_response(method: &Method) -> Response {
    match embedded("index.html") {
        Some(data) => {
            let mut response = bytes_response(data.to_vec(), method, "text/html; charset=utf-8");
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        None => {
            let mut response = bytes_response(MISSING_FRONTEND.as_bytes().to_vec(), method, "text/html; charset=utf-8");
            *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            response
        }
    }
}
