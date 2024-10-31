use super::Config;
use anyhow::Result;
use std::fs::File;

impl Config {
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        Ok(serde_yaml::from_str(yaml)?)
    }

    pub fn from_yaml_file(path: &str) -> Result<Self> {
        Ok(serde_yaml::from_reader(File::open(path)?)?)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn from_json_file(path: &str) -> Result<Self> {
        Ok(serde_json::from_reader(File::open(path)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, LambdaInvokeMode, PayloadMode, Target};
    use std::{
        collections::{HashMap, HashSet},
        io::Write,
        net::SocketAddr,
        sync::LazyLock,
    };
    use tempfile::NamedTempFile;

    static SIMPLE: LazyLock<Config> = LazyLock::new(|| Config {
        bind: SocketAddr::from(([0, 0, 0, 0], 8000)),
        targets: HashMap::from([(
            "/hello".to_string(),
            Target {
                function: "my-function".to_string(),
                payload: PayloadMode::ALB,
                invoke: LambdaInvokeMode::Buffered,
                auth: None,
            },
        )]),
    });
    static COMPLEX: LazyLock<Config> = LazyLock::new(|| Config {
        bind: SocketAddr::from(([0, 0, 0, 0], 8888)),
        targets: HashMap::from([(
            "/*wildcard".to_string(),
            Target {
                function: "arn:aws:lambda:us-east-2:123456789012:function:my-function:version".to_string(),
                payload: PayloadMode::ALB,
                invoke: LambdaInvokeMode::ResponseStream,
                auth: Some(AuthMode::ApiKeys(HashSet::from([
                    "key1".to_string(),
                    "key2".to_string(),
                ]))),
            },
        )]),
    });

    #[test]
    fn test_yaml() {
        let simple = r"
targets:
    /hello:
        function: my-function
        payload: alb
        ";
        let complex = r"
bind: 0.0.0.0:8888
targets:
    /*wildcard:
        function: arn:aws:lambda:us-east-2:123456789012:function:my-function:version
        payload: alb
        invoke: response_stream
        auth: !api_keys [key1, key2]
        ";

        assert_eq!(Config::from_yaml(simple).unwrap(), *SIMPLE);
        assert_eq!(Config::from_yaml(complex).unwrap(), *COMPLEX);

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", simple).unwrap();
        assert_eq!(Config::from_yaml_file(f.path().to_str().unwrap()).unwrap(), *SIMPLE);

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", complex).unwrap();
        assert_eq!(Config::from_yaml_file(f.path().to_str().unwrap()).unwrap(), *COMPLEX);
    }

    #[test]
    fn test_json() {
        let simple = r#"
{
  "targets": {
    "/hello": {
      "function": "my-function",
      "payload": "alb"
    }
  }
}
        "#;
        let complex = r#"
{
  "bind": "0.0.0.0:8888",
  "targets": {
    "/*wildcard": {
      "function": "arn:aws:lambda:us-east-2:123456789012:function:my-function:version",
      "payload": "alb",
      "invoke": "response_stream",
      "auth": {
        "api_keys": [
          "key1",
          "key2"
        ]
      }
    }
  }
}
        "#;

        assert_eq!(Config::from_json(simple).unwrap(), *SIMPLE);
        assert_eq!(Config::from_json(complex).unwrap(), *COMPLEX);

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", simple).unwrap();
        assert_eq!(Config::from_json_file(f.path().to_str().unwrap()).unwrap(), *SIMPLE);

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", complex).unwrap();
        assert_eq!(Config::from_json_file(f.path().to_str().unwrap()).unwrap(), *COMPLEX);
    }
}
