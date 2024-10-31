mod provider;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

/// There is no default payload mode yet.
/// Users must specify the payload mode explicitly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayloadMode {
    #[serde(rename = "alb")]
    ALB,
    // TODO: API Gateway HTTP API Proxy Event
    // TODO: API Gateway REST API Proxy Event
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum LambdaInvokeMode {
    #[default]
    #[serde(rename = "buffered")]
    Buffered,
    #[serde(rename = "response_stream")]
    ResponseStream,
}

/// [`Clone`] is not implemented to prevent accidental cloning.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMode {
    #[serde(rename = "api_keys")]
    ApiKeys(HashSet<String>),
}

/// [`Clone`] is not implemented to prevent accidental cloning.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Target {
    pub function: String,
    pub payload: PayloadMode,
    #[serde(default)]
    pub invoke: LambdaInvokeMode,
    #[serde(default)]
    pub auth: Option<AuthMode>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default = "Config::default_addr")]
    pub bind: SocketAddr,
    pub targets: HashMap<String, Target>,
}

impl Config {
    pub fn default_addr() -> SocketAddr {
        SocketAddr::from(([0, 0, 0, 0], 8000))
    }

    pub fn validate(self) -> anyhow::Result<Self> {
        // ensure targets not empty
        if self.targets.is_empty() {
            anyhow::bail!("targets is empty");
        }

        // ensure api_keys not empty
        for (name, target) in &self.targets {
            if let Some(AuthMode::ApiKeys(keys)) = &target.auth {
                if keys.is_empty() {
                    anyhow::bail!("api_keys is empty for target '{}'", name);
                }
            }
        }

        // ensure function name not empty
        for (name, target) in &self.targets {
            if target.function.is_empty() {
                anyhow::bail!("function name is empty for target '{}'", name);
            }
        }

        Ok(self)
    }
}
