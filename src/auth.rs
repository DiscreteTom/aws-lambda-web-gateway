use crate::config::{AuthMode, Config};
use axum::http::HeaderMap;

pub(super) fn is_authorized(headers: &HeaderMap, config: &Config) -> bool {
    match config.auth_mode {
        AuthMode::Open => true,
        AuthMode::ApiKey => {
            let api_key = headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .or_else(|| {
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok().and_then(|s| s.strip_prefix("Bearer ")))
                })
                .unwrap_or_default();

            config.api_keys.contains(api_key)
        }
    }
}
