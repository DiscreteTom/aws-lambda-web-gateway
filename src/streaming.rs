use crate::utils::handle_err;
use aws_sdk_lambda::{
    operation::invoke_with_response_stream::InvokeWithResponseStreamOutput,
    types::{InvokeResponseStreamUpdate, InvokeWithResponseStreamResponseEvent},
};
use axum::{
    body::Body,
    http::{response::Builder, HeaderMap, StatusCode},
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
    // collect metadata
    let (metadata, buffer) = {
        let mut buffer = vec![];
        loop {
            let next = handle_err!("Receiving response stream", resp.event_stream.recv().await);
            if let Some(PayloadChunk(InvokeResponseStreamUpdate {
                payload: Some(data), ..
            })) = next
            {
                buffer.extend_from_slice(&data.into_inner());

                // actually this is only required for the first chunk
                // but this is cheap, so we call it in the loop to simplify the flow
                if !detect_metadata(&buffer) {
                    break (None, buffer);
                }

                if let Some((prelude, remaining)) = try_parse_metadata(&mut buffer) {
                    break (Some(prelude), remaining.into());
                }
            } else {
                // no more chunks
                break (None, buffer);
            }
        }
    };

    let resp_builder = create_response_builder(metadata);

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

#[inline]
fn detect_metadata(bytes: &[u8]) -> bool {
    bytes.get(0) == Some(&b'{')
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

fn create_response_builder(metadata: Option<MetadataPrelude>) -> Builder {
    if let Some(metadata) = metadata {
        let mut builder = Response::builder().status(metadata.status_code);

        // apply all headers except content-length
        {
            let headers = builder.headers_mut().unwrap();
            *headers = metadata.headers;
            headers.remove("content-length");
        }

        for cookie in &metadata.cookies {
            builder = builder.header("set-cookie", cookie);
        }

        builder
    } else {
        // Default response if no metadata
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_metadata() {
        assert!(detect_metadata(b"{\"statusCode\":200,\"headers\":{}}"));
        assert!(!detect_metadata(b"Hello, world!"));
    }

    #[test]
    fn test_try_parse_metadata() {
        // incomplete
        assert!(try_parse_metadata(b"{\"statusCod").is_none());
        assert!(try_parse_metadata(b"{\"statusCode\":200,\"headers\":{}}\0\0\0").is_none());

        // complete
        let (metadata_prelude, remaining) =
            try_parse_metadata(b"{\"statusCode\":200,\"headers\":{}}\0\0\0\0\0\0\0\0Hello, world!").unwrap();
        assert_eq!(metadata_prelude.status_code, StatusCode::OK);
        assert_eq!(metadata_prelude.headers.len(), 0);
        assert_eq!(remaining, b"Hello, world!");
    }
}
