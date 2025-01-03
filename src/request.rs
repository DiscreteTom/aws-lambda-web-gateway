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
