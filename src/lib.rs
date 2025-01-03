mod config;
mod streaming;
mod utils;

use crate::config::{Config, LambdaInvokeMode};
use aws_config::BehaviorVersion;
use aws_lambda_events::{
    alb::{AlbTargetGroupRequest, AlbTargetGroupRequestContext, AlbTargetGroupResponse, ElbContext},
    query_map::QueryMap,
};
use aws_sdk_lambda::{operation::invoke::InvokeOutput, Client};
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
use std::{net::SocketAddr, sync::Arc};
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
    Query(query_string_parameters): Query<QueryMap>,
    State(state): State<ApplicationState>,
    http_method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let client = &state.client;
    let config = &state.config;
    let path = "/".to_string() + path.map(|p| p.0).unwrap_or_default().as_str();

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

    let lambda_request_body = handle_err!(
        "Building lambda request",
        serde_json::to_string(&AlbTargetGroupRequest {
            http_method,
            headers,
            path: path.into(),
            query_string_parameters,
            body: body.into(),
            is_base64_encoded,
            request_context: AlbTargetGroupRequestContext {
                elb: ElbContext { target_group_arn: None },
            },
            // TODO: support multi-value-header mode?
            multi_value_headers: Default::default(),
            multi_value_query_string_parameters: Default::default(),
        })
    );

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
                    .payload(Blob::new(lambda_request_body))
                    .send()
                    .await
            );
            handle_streaming_response(resp).await
        }
    }
}

async fn handle_buffered_response(resp: InvokeOutput) -> Response {
    // Parse the InvokeOutput payload to extract the LambdaResponse
    let payload = resp.payload().map_or(&[] as &[u8], |v| v.as_ref());
    let lambda_response = handle_err!(
        "Deserializing lambda response",
        serde_json::from_slice::<AlbTargetGroupResponse>(payload)
    );

    // Build the response using the extracted information
    let mut resp_builder = Response::builder().status(handle_err!(
        "Parse response status code",
        StatusCode::from_u16(handle_err!(
            "Parse response status code",
            lambda_response.status_code.try_into()
        ))
    ));

    *handle_err!(
        "Setting response headers",
        resp_builder.headers_mut().ok_or("Errors in builder")
    ) = lambda_response.headers;

    let mut body = lambda_response.body.map_or(vec![], |b| b.to_vec());
    if lambda_response.is_base64_encoded {
        body = handle_err!("Decoding base64 body", BASE64_STANDARD.decode(body));
    }
    handle_err!("Building response", resp_builder.body(Body::from(body)))
}

#[cfg(test)]
mod tests;
