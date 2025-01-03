mod auth;
mod buffered;
mod config;
mod request;
mod streaming;
mod utils;

use crate::config::{Config, LambdaInvokeMode};
use auth::is_authorized;
use aws_config::BehaviorVersion;
use aws_lambda_events::query_map::QueryMap;
use aws_sdk_lambda::Client;
use aws_smithy_types::Blob;
use axum::{
    body::{Body, Bytes},
    extract::{Query, State},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use buffered::handle_buffered_response;
use request::build_alb_request_body;
use std::{net::SocketAddr, sync::Arc};
use streaming::handle_streaming_response;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use utils::{handle_err, transform_body, whether_base64_encoded};

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
    State(state): State<ApplicationState>,
    Query(query): Query<QueryMap>,
    parts: Parts,
    body: Bytes,
) -> Response {
    if !is_authorized(&parts.headers, &state.config) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let is_base64_encoded = whether_base64_encoded(&parts.headers);
    let body = transform_body(is_base64_encoded, body);

    let lambda_request_body = handle_err!(
        "Building lambda request",
        build_alb_request_body(is_base64_encoded, query, parts, body)
    );

    macro_rules! call_lambda {
        ($action:ident) => {
            handle_err!(
                "Invoking lambda",
                state
                    .client
                    .$action()
                    .function_name(state.config.lambda_function_name.as_str())
                    .payload(Blob::new(lambda_request_body))
                    .send()
                    .await
            )
        };
    }

    match state.config.lambda_invoke_mode {
        LambdaInvokeMode::Buffered => handle_buffered_response(call_lambda!(invoke)).await,
        LambdaInvokeMode::ResponseStream => handle_streaming_response(call_lambda!(invoke_with_response_stream)).await,
    }
}

#[cfg(test)]
mod tests;
