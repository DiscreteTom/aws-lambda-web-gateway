use aws_lambda_events::{
    alb::{AlbTargetGroupRequest, AlbTargetGroupRequestContext, ElbContext},
    query_map::QueryMap,
};
use axum::http::request::Parts;

pub(super) fn build_alb_request_body(
    is_base64_encoded: bool,
    query_string_parameters: QueryMap,
    parts: Parts,
    body: String,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&AlbTargetGroupRequest {
        http_method: parts.method,
        headers: parts.headers,
        path: parts.uri.path().to_string().into(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{request::Builder, Method};
    use base64::{prelude::BASE64_STANDARD, Engine};
    use std::collections::HashMap;

    // TODO:update aws_lambda_events to make these tests pass
    // https://github.com/awslabs/aws-lambda-rust-runtime/issues/954

    #[test]
    fn test_alb_body() {
        let (parts, body) = Builder::new()
            .method(Method::GET)
            .uri("https://example.com/?k=v")
            .header("key", "value")
            .body("Hello, world!")
            .unwrap()
            .into_parts();
        let query = HashMap::from([("k".to_string(), "v".to_string())]).into();

        let expected = "{\"httpMethod\":\"GET\",\"path\":\"/\",\"queryStringParameters\":{\"k\":\"v\"},\"multiValueQueryStringParameters\":{},\"headers\":{\"key\":\"value\"},\"multiValueHeaders\":{},\"requestContext\":{\"elb\":{\"targetGroupArn\":null}},\"isBase64Encoded\":false,\"body\":\"Hello, world!\"}";
        assert_eq!(
            build_alb_request_body(false, query, parts, body.into()).unwrap(),
            expected
        );
    }

    #[test]
    fn test_alb_body_base64() {
        let (parts, body) = Builder::new()
            .method(Method::GET)
            .uri("https://example.com/?k=v")
            .header("key", "value")
            .body(BASE64_STANDARD.encode("Hello, world!"))
            .unwrap()
            .into_parts();
        let query = HashMap::from([("k".to_string(), "v".to_string())]).into();

        let expected = "{\"httpMethod\":\"GET\",\"path\":\"/\",\"queryStringParameters\":{\"k\":\"v\"},\"multiValueQueryStringParameters\":{},\"headers\":{\"key\":\"value\"},\"multiValueHeaders\":{},\"requestContext\":{\"elb\":{\"targetGroupArn\":null}},\"isBase64Encoded\":true,\"body\":\"SGVsbG8sIHdvcmxkIQ==\"}";
        assert_eq!(build_alb_request_body(true, query, parts, body).unwrap(), expected);
    }
}
