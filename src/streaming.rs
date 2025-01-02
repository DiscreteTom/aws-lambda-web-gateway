use crate::utils::handle_err;
use aws_sdk_lambda::{
    operation::invoke_with_response_stream::InvokeWithResponseStreamOutput,
    types::{InvokeResponseStreamUpdate, InvokeWithResponseStreamResponseEvent},
};
use aws_smithy_types::Blob;
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use InvokeWithResponseStreamResponseEvent::*;

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

pub(super) async fn handle_streaming_response(mut resp: InvokeWithResponseStreamOutput) -> Response {
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

        while let Ok(Some(event)) = resp.event_stream.recv().await {
            tx.send(event).await.ok();
        }
    });

    let stream = ReceiverStream::new(rx).map(|event| match event {
        PayloadChunk(chunk) => {
            if let Some(data) = chunk.payload {
                let bytes = data.into_inner();
                Ok::<_, Infallible>(Bytes::from(bytes))
            } else {
                Ok(Bytes::default())
            }
        }
        InvokeComplete(_) => Ok(Bytes::default()),
        _ => {
            tracing::warn!("Unhandled event type: {:?}", event);
            Ok(Bytes::default())
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

    handle_err!("Building response", resp_builder.body(Body::from_stream(stream)))
}

async fn detect_metadata(resp: &mut InvokeWithResponseStreamOutput) -> (bool, Option<Vec<u8>>) {
    if let Ok(Some(PayloadChunk(chunk))) = resp.event_stream.recv().await {
        if let Some(data) = chunk.payload {
            let bytes = data.into_inner();
            let has_metadata = !bytes.is_empty() && bytes[0] == b'{';
            return (has_metadata, Some(bytes));
        }
    }
    (false, None)
}

async fn collect_metadata(
    resp: &mut InvokeWithResponseStreamOutput,
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
            if let Some(data) = chunk.payload {
                let bytes = data.into_inner();
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
