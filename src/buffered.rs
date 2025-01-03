use crate::utils::handle_err;
use aws_lambda_events::alb::AlbTargetGroupResponse;
use aws_sdk_lambda::operation::invoke::InvokeOutput;
use axum::{body::Body, http::StatusCode, response::Response};
use base64::{prelude::BASE64_STANDARD, Engine};

pub(super) async fn handle_buffered_response(resp: InvokeOutput) -> Response {
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