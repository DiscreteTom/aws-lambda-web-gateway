use super::*;
use std::collections::HashSet;

#[test]
fn test_deserialize_payload_mode() {
    // ALB
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb")
            .unwrap()
            .payload,
        PayloadMode::ALB
    );
    // invalid
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: invalid")
            .unwrap_err()
            .to_string(),
        "payload: unknown variant `invalid`, expected `alb` at line 2 column 10"
    );
    // missing
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test")
            .unwrap_err()
            .to_string(),
        "missing field `payload`"
    );
}

#[test]
fn test_default_invoke_mode() {
    assert_eq!(LambdaInvokeMode::default(), LambdaInvokeMode::Buffered);
}

#[test]
fn test_deserialize_invoke_mode() {
    // buffered
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb\ninvoke: buffered")
            .unwrap()
            .invoke,
        LambdaInvokeMode::Buffered
    );
    // response_stream
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb\ninvoke: response_stream")
            .unwrap()
            .invoke,
        LambdaInvokeMode::ResponseStream
    );
    // default
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb")
            .unwrap()
            .invoke,
        LambdaInvokeMode::Buffered
    );
    // invalid
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb\ninvoke: invalid")
            .unwrap_err()
            .to_string(),
        "invoke: unknown variant `invalid`, expected `buffered` or `response_stream` at line 3 column 9"
    );
}

#[test]
fn test_deserialize_auth_mode() {
    // api_keys
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb\nauth: !api_keys [a, b]")
            .unwrap()
            .auth,
        Some(AuthMode::ApiKeys(HashSet::from(["a".to_string(), "b".to_string()])))
    );
    // invalid api_keys
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb\nauth: !api_keys abc")
            .unwrap_err()
            .to_string(),
        "auth: invalid type: string \"abc\", expected a sequence at line 3 column 7"
    );
    // default
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb")
            .unwrap()
            .auth,
        None
    );
    // invalid
    assert_eq!(
        serde_yaml::from_str::<Target>("function: test\npayload: alb\nauth: !invalid []")
            .unwrap_err()
            .to_string(),
        "unknown variant `invalid`, expected `api_keys`"
    );
}

#[test]
fn test_deserialize_bind() {
    // custom
    assert_eq!(
        serde_yaml::from_str::<Config>("targets: {}\nbind: 0.0.0.0:8888")
            .unwrap()
            .bind,
        SocketAddr::from(([0, 0, 0, 0], 8888))
    );
    // default
    assert_eq!(
        serde_yaml::from_str::<Config>("targets: {}").unwrap().bind,
        SocketAddr::from(([0, 0, 0, 0], 8000))
    );
    // invalid
    assert_eq!(
        serde_yaml::from_str::<Config>("targets: {}\nbind: invalid")
            .unwrap_err()
            .to_string(),
        "bind: invalid socket address syntax at line 2 column 7"
    );
}

#[test]
fn test_validate() {
    // empty targets
    assert_eq!(
        Config {
            bind: SocketAddr::from(([0, 0, 0, 0], 8000)),
            targets: HashMap::new()
        }
        .validate()
        .unwrap_err()
        .to_string(),
        "targets is empty"
    );
    // empty api_keys
    assert_eq!(
        Config {
            bind: SocketAddr::from(([0, 0, 0, 0], 8000)),
            targets: HashMap::from([(
                "test".to_string(),
                Target {
                    function: "test".to_string(),
                    payload: PayloadMode::ALB,
                    invoke: LambdaInvokeMode::Buffered,
                    auth: Some(AuthMode::ApiKeys(HashSet::new()))
                }
            )]),
        }
        .validate()
        .unwrap_err()
        .to_string(),
        "api_keys is empty for target 'test'"
    );
    // empty function name
    assert_eq!(
        Config {
            bind: SocketAddr::from(([0, 0, 0, 0], 8000)),
            targets: HashMap::from([(
                "test".to_string(),
                Target {
                    function: "".to_string(),
                    payload: PayloadMode::ALB,
                    invoke: LambdaInvokeMode::Buffered,
                    auth: None
                }
            )]),
        }
        .validate()
        .unwrap_err()
        .to_string(),
        "function name is empty for target 'test'"
    );
    // valid
    assert!(Config {
        bind: SocketAddr::from(([0, 0, 0, 0], 8000)),
        targets: HashMap::from([(
            "test".to_string(),
            Target {
                function: "test".to_string(),
                payload: PayloadMode::ALB,
                invoke: LambdaInvokeMode::Buffered,
                auth: None
            }
        )]),
    }
    .validate()
    .is_ok());
}
