#[cfg(test)]
mod tests;

use crate::config::{Config, LambdaInvokeMode};
use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_lambda::{
    types::{
        InvokeResponseStreamUpdate,
        InvokeWithResponseStreamResponseEvent::{InvokeComplete, PayloadChunk},
        ResponseStreamingInvocationType,
    },
    Client,
};
use aws_smithy_types::Blob;
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, MethodRouter},
    Router,
};
use base64::Engine;
use config::{AuthMode, PayloadMode, Target};
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, env, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::trace::TraceLayer;

pub mod config;

#[derive(Clone)]
pub struct ApplicationState {
    client: Client,
}

pub fn read_config(default_path: &str) -> Result<Config> {
    if let Ok(line) = env::var("AWS_LWG_INLINE_JSON_CONFIG") {
        tracing::trace!("applying inline json config: {}", line);
        Config::from_json(&line)
    } else {
        let path = env::var("AWS_LWG_CONFIG_PATH");
        let path = path.as_deref().unwrap_or(default_path);
        tracing::trace!("applying config path: {}", path);
        if path.ends_with(".json") {
            Config::from_json_file(path)
        } else if path.ends_with(".yml") || path.ends_with(".yaml") {
            Config::from_yaml_file(path)
        } else {
            anyhow::bail!("Unsupported config file format")
        }
    }
}

pub async fn run_app() {
    tracing_subscriber::fmt::init();

    let config = read_config("config.yaml").unwrap().validate().unwrap();
    tracing::debug!("applied config: {:?}", config);

    let mut app = Router::new().route("/healthz", get(health));
    for (path, target) in config.targets.into_iter() {
        app = app.route(&path, handler_factory(target));
    }
    let app = app.layer(TraceLayer::new_for_http()).with_state({
        let aws_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        let client = Client::new(&aws_config);
        ApplicationState { client }
    });

    let listener = tokio::net::TcpListener::bind(&config.bind).await.unwrap();
    tracing::info!("Listening on {}", config.bind);
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> impl IntoResponse {
    StatusCode::OK
}

fn handler_factory(target: Target) -> MethodRouter<ApplicationState> {
    let target = Arc::new(target); // make it cheap to clone
    any(move |State(state): State<ApplicationState>, req: Request| async move {
        let client = &state.client;

        let (parts, body) = req.into_parts();
        let http_method = parts.method.as_str();
        let headers = parts.headers;
        let path = parts.uri.path();
        let Query(query) = Query::<HashMap<String, String>>::try_from_uri(&parts.uri).unwrap();
        let body = to_bytes(body, usize::MAX).await.unwrap();

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
            base64::engine::general_purpose::STANDARD.encode(body)
        } else {
            String::from_utf8_lossy(&body).to_string()
        };

        if let Some(auth) = &target.auth {
            match auth {
                AuthMode::ApiKeys(keys) => {
                    let api_key = headers
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .or_else(|| {
                            headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok().and_then(|s| s.strip_prefix("Bearer ")))
                        })
                        .unwrap_or_default();

                    if !keys.contains(api_key) {
                        return Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Body::empty())
                            .unwrap();
                    }
                }
            }
        }

        let lambda_request_body = match &target.payload {
            PayloadMode::ALB => json!({
                "httpMethod": http_method,
                "headers": to_string_map(&headers),
                "path": path,
                "queryStringParameters": query,
                "isBase64Encoded": is_base64_encoded,
                "body": body,
                "requestContext": {
                    "elb": {
                        "targetGroupArn": "",
                    },
                },
            }),
        }
        .to_string();

        match target.invoke {
            LambdaInvokeMode::Buffered => {
                let resp = client
                    .invoke()
                    .function_name(target.function.as_str())
                    .payload(Blob::new(lambda_request_body))
                    .send()
                    .await
                    .unwrap();
                handle_buffered_response(resp).await
            }
            LambdaInvokeMode::ResponseStream => {
                let resp = client
                    .invoke_with_response_stream()
                    .function_name(target.function.as_str())
                    .invocation_type(ResponseStreamingInvocationType::RequestResponse)
                    .payload(Blob::new(lambda_request_body))
                    .send()
                    .await
                    .unwrap();
                handle_streaming_response(resp).await
            }
        }
    })
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

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataPrelude {
    #[serde(with = "http_serde::status_code")]
    /// The HTTP status code.
    pub status_code: StatusCode,
    #[serde(with = "http_serde::header_map")]
    /// The HTTP headers.
    pub headers: HeaderMap,
    /// The HTTP cookies.
    pub cookies: Vec<String>,
}

async fn handle_buffered_response(resp: aws_sdk_lambda::operation::invoke::InvokeOutput) -> Response {
    // Parse the InvokeOutput payload to extract the LambdaResponse
    let payload = resp.payload().unwrap().as_ref().to_vec();
    let lambda_response: LambdaResponse = serde_json::from_slice(&payload).unwrap();

    // Build the response using the extracted information
    let mut resp_builder = Response::builder().status(StatusCode::from_u16(lambda_response.status_code).unwrap());

    if let Some(headers) = lambda_response.headers {
        for (key, value) in headers {
            resp_builder = resp_builder.header(key, value);
        }
    }

    let body = if lambda_response.is_base64_encoded.unwrap_or(false) {
        base64::engine::general_purpose::STANDARD
            .decode(lambda_response.body)
            .unwrap()
    } else {
        lambda_response.body.into_bytes()
    };
    resp_builder.body(Body::from(body)).unwrap()
}

async fn handle_streaming_response(
    mut resp: aws_sdk_lambda::operation::invoke_with_response_stream::InvokeWithResponseStreamOutput,
) -> Response {
    let (tx, rx) = mpsc::channel(1);
    let mut metadata_buffer = Vec::new();
    let mut metadata_prelude: Option<MetadataPrelude> = None;
    let mut remaining_data = Vec::new();

    // Step 1: Detect if metadata exists and get the first chunk
    let (has_metadata, first_chunk) = detect_metadata(&mut resp).await;

    // Step 2: Process the first chunk
    if let Some(chunk) = first_chunk {
        if has_metadata {
            metadata_buffer.extend_from_slice(&chunk);
            (metadata_prelude, remaining_data) = collect_metadata(&mut resp, &mut metadata_buffer).await;
        } else {
            // No metadata prelude, treat first chunk as payload
            remaining_data = chunk;
        }
    }

    // Spawn task to handle remaining stream
    tokio::spawn(async move {
        // Send remaining data after metadata first
        if !remaining_data.is_empty() {
            let stream_update = InvokeResponseStreamUpdate::builder()
                .payload(Blob::new(remaining_data))
                .build();
            let _ = tx.send(PayloadChunk(stream_update)).await;
        }

        while let Some(event) = resp.event_stream.recv().await.unwrap() {
            match event {
                PayloadChunk(chunk) => {
                    if let Some(data) = chunk.payload() {
                        let stream_update = InvokeResponseStreamUpdate::builder().payload(data.clone()).build();
                        let _ = tx.send(PayloadChunk(stream_update)).await;
                    }
                }
                InvokeComplete(_) => {
                    let _ = tx.send(event).await;
                }
                _ => {}
            }
        }
    });

    let stream = ReceiverStream::new(rx).map(|event| {
        match event {
            PayloadChunk(chunk) => {
                if let Some(data) = chunk.payload() {
                    let bytes = data.clone().into_inner();
                    Ok::<_, std::convert::Infallible>(Bytes::from(bytes))
                } else {
                    Ok(Bytes::default())
                }
            }
            InvokeComplete(_) => Ok(Bytes::default()),
            _ => Ok(Bytes::default()), // Handle other event types
        }
    });

    let mut resp_builder = Response::builder();

    if let Some(metadata_prelude) = metadata_prelude {
        resp_builder = resp_builder.status(metadata_prelude.status_code);

        for (k, v) in metadata_prelude.headers.iter() {
            if k != "content-length" {
                resp_builder = resp_builder.header(k, v);
            }
        }

        for cookie in &metadata_prelude.cookies {
            resp_builder = resp_builder.header("set-cookie", cookie);
        }
    } else {
        // Default response if no metadata
        resp_builder = resp_builder.status(StatusCode::OK);
        resp_builder = resp_builder.header("content-type", "application/octet-stream");
    }

    resp_builder.body(Body::from_stream(stream)).unwrap()
}

async fn detect_metadata(
    resp: &mut aws_sdk_lambda::operation::invoke_with_response_stream::InvokeWithResponseStreamOutput,
) -> (bool, Option<Vec<u8>>) {
    if let Ok(Some(PayloadChunk(chunk))) = resp.event_stream.recv().await {
        if let Some(data) = chunk.payload() {
            let bytes = data.clone().into_inner();
            let has_metadata = !bytes.is_empty() && bytes[0] == b'{';
            return (has_metadata, Some(bytes));
        }
    }
    (false, None)
}

async fn collect_metadata(
    resp: &mut aws_sdk_lambda::operation::invoke_with_response_stream::InvokeWithResponseStreamOutput,
    metadata_buffer: &mut Vec<u8>,
) -> (Option<MetadataPrelude>, Vec<u8>) {
    let mut metadata_prelude = None;
    let mut remaining_data = Vec::new();

    // Process the metadata_buffer first
    let (prelude, remaining) = process_buffer(metadata_buffer);
    if let Some(p) = prelude {
        return (Some(p), remaining);
    }

    // If metadata is not complete, continue processing the stream
    while let Ok(Some(event)) = resp.event_stream.recv().await {
        if let PayloadChunk(chunk) = event {
            if let Some(data) = chunk.payload() {
                let bytes = data.clone().into_inner();
                metadata_buffer.extend_from_slice(&bytes);
                let (prelude, remaining) = process_buffer(metadata_buffer);
                if let Some(p) = prelude {
                    metadata_prelude = Some(p);
                    remaining_data = remaining;
                    break;
                }
            }
        }
    }
    (metadata_prelude, remaining_data)
}

fn process_buffer(buffer: &[u8]) -> (Option<MetadataPrelude>, Vec<u8>) {
    let mut null_count = 0;
    for (i, &byte) in buffer.iter().enumerate() {
        if byte == 0 {
            null_count += 1;
            if null_count == 8 {
                let metadata_str = String::from_utf8_lossy(&buffer[..i]);
                let metadata_prelude = serde_json::from_str(&metadata_str).unwrap_or_default();
                tracing::debug!(metadata_prelude=?metadata_prelude);
                // Save remaining data after metadata
                let remaining_data = buffer[i + 1..].to_vec();
                return (Some(metadata_prelude), remaining_data);
            }
        } else {
            null_count = 0;
        }
    }
    (None, Vec::new())
}
