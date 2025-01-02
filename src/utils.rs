macro_rules! handle_err {
    ($name:expr, $result:expr) => {{
        match $result {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("{}: {:?}", $name, e);
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap();
            }
        }
    }};
}
pub(crate) use handle_err;
