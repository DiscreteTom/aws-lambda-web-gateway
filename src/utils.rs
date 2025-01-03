use axum::http::HeaderMap;
use base64::{prelude::BASE64_STANDARD, Engine};
use bytes::Bytes;

macro_rules! handle_err {
    ($name:expr, $result:expr) => {{
        match $result {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("{}: {:?}", $name, e);
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap();
            }
        }
    }};
}
pub(crate) use handle_err;

pub(super) fn whether_base64_encoded(headers: &HeaderMap) -> bool {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    match content_type {
        "application/json" => false,
        "application/xml" => false,
        "application/javascript" => false,
        _ if content_type.starts_with("text/") => false,
        _ => true,
    }
}

pub(super) fn transform_body(is_base64_encoded: bool, body: Bytes) -> String {
    if is_base64_encoded {
        BASE64_STANDARD.encode(body)
    } else {
        String::from_utf8_lossy(&body).to_string()
    }
}
