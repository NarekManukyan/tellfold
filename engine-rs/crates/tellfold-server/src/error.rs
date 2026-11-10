//! HTTP errors in FastAPI's shapes: `{"detail": "text"}` for an `HTTPException`, `{"detail": [...]}` (status 422)
//! for a request that fails validation.

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{json, Value};

/// JSON as FastAPI writes a response model: pydantic's compact `model_dump_json()` text.
pub fn json_response<T: Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    let mut response = Response::new(Body::from(tellfold_models::json::to_string(value)));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

pub fn ok_json<T: Serialize + ?Sized>(value: &T) -> Response {
    json_response(StatusCode::OK, value)
}

/// An error response: `HTTPException(status, detail)`, or a validation failure.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub detail: Value,
    pub headers: Vec<(header::HeaderName, HeaderValue)>,
}

impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::from_u16(status).expect("a valid status code"),
            detail: Value::String(detail.into()),
            headers: Vec::new(),
        }
    }

    /// 422 with pydantic's error list.
    pub fn validation(errors: Vec<Value>) -> Self {
        ApiError { status: StatusCode::UNPROCESSABLE_ENTITY, detail: Value::Array(errors), headers: Vec::new() }
    }

    /// An unexpected failure: Starlette's plain-text 500.
    pub fn internal(error: impl std::fmt::Display) -> Self {
        crate::log::warning("tellfold.server", &format!("request failed: {error}"));
        ApiError { status: StatusCode::INTERNAL_SERVER_ERROR, detail: Value::Null, headers: Vec::new() }
    }

    pub fn with_header(mut self, name: header::HeaderName, value: &'static str) -> Self {
        self.headers.push((name, HeaderValue::from_static(value)));
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = if self.detail.is_null() {
            let mut r = Response::new(Body::from("Internal Server Error"));
            *r.status_mut() = self.status;
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            r
        } else {
            json_response(self.status, &json!({"detail": self.detail}))
        };
        for (name, value) in self.headers {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

pub type ApiResult = Result<Response, ApiError>;
