use crate::utils::handle_err;
use aws_sdk_lambda::{
    operation::invoke_with_response_stream::InvokeWithResponseStreamOutput,
    types::{InvokeResponseStreamUpdate, InvokeWithResponseStreamResponseEvent},
};
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

// TODO: contribute to `lambda_runtime` crate to make this struct derive Deserialize
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataPrelude {
    /// The HTTP status code.
    #[serde(with = "http_serde::status_code")]
    pub status_code: StatusCode,
    /// The HTTP headers.
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
    /// The HTTP cookies.
    pub cookies: Vec<String>,
}

pub(super) async fn handle_streaming_response(mut resp: InvokeWithResponseStreamOutput) -> Response {
    let Some(PayloadChunk(InvokeResponseStreamUpdate {
        payload: Some(first_chunk),
        ..
    })) = handle_err!("Receiving response stream", resp.event_stream.recv().await)
    else {
        // TODO: correct the response if there is no chunk
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .unwrap();
    };
    let mut buffer = first_chunk.into_inner();

    // Detect and collect metadata prelude
    let (metadata_prelude, buffer) = if detect_metadata(&buffer) {
        if let Some((metadata_prelude, rest)) = collect_metadata(&mut resp, &mut buffer).await {
            (Some(metadata_prelude), rest)
        } else {
            (None, buffer)
        }
    } else {
        (None, buffer)
    };

    let mut resp_builder = Response::builder();

    if let Some(metadata_prelude) = metadata_prelude {
        resp_builder = resp_builder.status(metadata_prelude.status_code);

        {
            let headers = resp_builder.headers_mut().unwrap();
            *headers = metadata_prelude.headers;
            headers.remove("content-length");
        }

        for cookie in &metadata_prelude.cookies {
            resp_builder = resp_builder.header("set-cookie", cookie);
        }
    } else {
        // Default response if no metadata
        resp_builder = resp_builder.status(StatusCode::OK);
        resp_builder = resp_builder.header("content-type", "application/octet-stream");
    }

    // Spawn task to handle remaining stream
    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        // Send remaining data after metadata first
        if !buffer.is_empty() {
            tx.send(buffer).await.ok();
        }

        // TODO: handle error
        while let Ok(Some(event)) = resp.event_stream.recv().await {
            match event {
                PayloadChunk(chunk) => {
                    if let Some(data) = chunk.payload {
                        tx.send(data.into_inner()).await.ok();
                    }
                    // else, no data in the chunk, just ignore
                }
                InvokeComplete(_) => {
                    break;
                }
                _ => {
                    tracing::warn!("Unhandled event type: {:?}", event);
                }
            }
        }
    });

    handle_err!(
        "Building response",
        resp_builder.body(Body::from_stream(ReceiverStream::new(rx).map(|bytes| Ok::<
            _,
            Infallible,
        >(
            Bytes::from(bytes)
        ))))
    )
}

fn detect_metadata(bytes: &[u8]) -> bool {
    bytes.get(0) == Some(&b'{')
}

/// Return metadata prelude and remaining data if metadata is complete.
/// Return [`None`] if the stream is exhausted without complete metadata.
async fn collect_metadata(
    resp: &mut InvokeWithResponseStreamOutput,
    metadata_buffer: &mut Vec<u8>,
) -> Option<(MetadataPrelude, Vec<u8>)> {
    // Process the metadata_buffer first
    if let Some((prelude, remaining)) = try_parse_metadata(metadata_buffer) {
        return Some((prelude, remaining.into()));
    }

    // If metadata is not complete, continue processing the stream
    // TODO: handle error
    while let Ok(Some(PayloadChunk(InvokeResponseStreamUpdate {
        payload: Some(data), ..
    }))) = resp.event_stream.recv().await
    {
        let bytes = data.into_inner();
        metadata_buffer.extend_from_slice(&bytes);
        if let Some((prelude, remaining)) = try_parse_metadata(metadata_buffer) {
            return Some((prelude, remaining.into()));
        }
    }
    None
}

/// If metadata prelude is found, return the metadata prelude and the remaining data.
fn try_parse_metadata(buffer: &[u8]) -> Option<(MetadataPrelude, &[u8])> {
    let mut null_count = 0;
    for (i, &byte) in buffer.iter().enumerate() {
        if byte != 0 {
            null_count = 0;
            continue;
        }
        null_count += 1;
        if null_count == 8 {
            // now we have 8 continuous null bytes
            let metadata_str = String::from_utf8_lossy(&buffer[..i - 7]);
            // TODO: handle invalid metadata prelude
            let metadata_prelude = serde_json::from_str(&metadata_str).unwrap_or_default();
            tracing::debug!(metadata_prelude=?metadata_prelude);
            return Some((metadata_prelude, &buffer[i + 1..]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_metadata() {
        assert!(detect_metadata(b"{\"statusCode\":200,\"headers\":{}}"));
        assert!(!detect_metadata(b"Hello, world!"));
    }
}
