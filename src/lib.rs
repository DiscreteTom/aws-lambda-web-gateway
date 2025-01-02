mod config;
mod streaming;
mod utils;

use crate::config::{Config, LambdaInvokeMode};
use aws_config::BehaviorVersion;
use aws_sdk_lambda::{operation::invoke::InvokeOutput, types::ResponseStreamingInvocationType, Client};
use aws_smithy_types::Blob;
use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use base64::{prelude::BASE64_STANDARD, Engine};
use config::AuthMode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use streaming::handle_streaming_response;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use utils::handle_err;

#[derive(Clone)]
pub struct ApplicationState {
    client: Client,
    config: Arc<Config>,
}

pub async fn run_app() {
    tracing_subscriber::fmt::init();

    let config = Arc::new(Config::load("config.yaml"));
    let aws_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = Client::new(&aws_config);

    let app_state = ApplicationState { client, config };
    let addr = app_state.config.addr.parse::<SocketAddr>().unwrap();

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/", any(handler))
        .route("/*path", any(handler))
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

    let listener = TcpListener::bind(addr).await.unwrap();
    tracing::info!("Listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> impl IntoResponse {
    StatusCode::OK
}

async fn handler(
    path: Option<Path<String>>,
    Query(query_string_parameters): Query<HashMap<String, String>>,
    State(state): State<ApplicationState>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let client = &state.client;
    let config = &state.config;
    let path = "/".to_string() + path.map(|p| p.0).unwrap_or_default().as_str();

    let http_method = method.to_string();

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    let is_base64_encoded = match content_type {
        "application/json" => false,
        "application/xml" => false,
        "application/javascript" => false,
        _ if content_type.starts_with("text/") => false,
        _ => true,
    };

    let body = if is_base64_encoded {
        BASE64_STANDARD.encode(body)
    } else {
        String::from_utf8_lossy(&body).to_string()
    };

    match config.auth_mode {
        AuthMode::Open => {}
        AuthMode::ApiKey => {
            let api_key = headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .or_else(|| {
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok().and_then(|s| s.strip_prefix("Bearer ")))
                })
                .unwrap_or_default();

            if !config.api_keys.contains(api_key) {
                return Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(Body::empty())
                    .unwrap();
            }
        }
    }

    let lambda_request_body = json!({
        "httpMethod": http_method,
        "headers": to_string_map(&headers),
        "path": path,
        "queryStringParameters": query_string_parameters,
        "isBase64Encoded": is_base64_encoded,
        "body": body,
        "requestContext": {
            "elb": {
                "targetGroupArn": "",
            },
        },
    })
    .to_string();

    match config.lambda_invoke_mode {
        LambdaInvokeMode::Buffered => {
            let resp = handle_err!(
                "Invoking lambda",
                client
                    .invoke()
                    .function_name(config.lambda_function_name.as_str())
                    .payload(Blob::new(lambda_request_body))
                    .send()
                    .await
            );
            handle_buffered_response(resp).await
        }
        LambdaInvokeMode::ResponseStream => {
            let resp = handle_err!(
                "Invoking lambda",
                client
                    .invoke_with_response_stream()
                    .function_name(config.lambda_function_name.as_str())
                    .invocation_type(ResponseStreamingInvocationType::RequestResponse)
                    .payload(Blob::new(lambda_request_body))
                    .send()
                    .await
            );
            handle_streaming_response(resp).await
        }
    }
}

fn to_string_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_owned(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect()
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct LambdaResponse {
    status_code: u16,
    status_description: Option<String>,
    is_base64_encoded: Option<bool>,
    headers: Option<HashMap<String, String>>,
    body: String,
}

async fn handle_buffered_response(resp: InvokeOutput) -> Response {
    // Parse the InvokeOutput payload to extract the LambdaResponse
    let payload = resp.payload().map_or(&[] as &[u8], |v| v.as_ref());
    let lambda_response = handle_err!(
        "Deserializing lambda response",
        serde_json::from_slice::<LambdaResponse>(payload)
    );

    // Build the response using the extracted information
    let mut resp_builder = Response::builder().status(StatusCode::from_u16(lambda_response.status_code).unwrap());

    if let Some(headers) = lambda_response.headers {
        for (key, value) in headers {
            resp_builder = resp_builder.header(key, value);
        }
    }

    let body = if lambda_response.is_base64_encoded.unwrap_or(false) {
        handle_err!(
            "Decode base64 lambda response body",
            BASE64_STANDARD.decode(lambda_response.body)
        )
    } else {
        lambda_response.body.into_bytes()
    };
    handle_err!("Building response", resp_builder.body(Body::from(body)))
}

#[cfg(test)]
mod tests;
