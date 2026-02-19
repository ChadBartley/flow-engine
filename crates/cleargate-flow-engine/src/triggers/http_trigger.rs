//! HTTP webhook trigger.
//!
//! Does **not** start its own HTTP server. Provides route configurations
//! that the API layer (M10) mounts as axum handlers.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use super::super::errors::TriggerError;
use super::super::traits::Trigger;
use super::super::types::TriggerEvent;

/// A route definition for the API layer to mount.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HttpTriggerRoute {
    /// URL path, e.g. "/webhook/enrich".
    pub path: String,
    /// HTTP method, e.g. "POST".
    pub method: String,
    /// The flow this route triggers.
    pub flow_id: String,
}

/// HTTP webhook trigger.
///
/// The trigger itself is passive — it provides route configs via [`routes()`](HttpTrigger::routes)
/// and the API layer mounts them. `start()` simply awaits the shutdown signal.
pub struct HttpTrigger;

impl HttpTrigger {
    /// Parse the config to extract route definitions.
    ///
    /// Config shape: `{ "path": "/webhook/enrich", "method": "POST", "flow_id": "my-flow" }`
    pub fn routes(&self, config: &Value) -> Result<Vec<HttpTriggerRoute>, TriggerError> {
        let path =
            config
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TriggerError::Config {
                    message: "missing 'path' in config".into(),
                })?;

        if !path.starts_with('/') {
            return Err(TriggerError::Config {
                message: format!("path must start with '/': {path}"),
            });
        }

        let method = config
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("POST");

        let flow_id = config
            .get("flow_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TriggerError::Config {
                message: "missing 'flow_id' in config".into(),
            })?;

        Ok(vec![HttpTriggerRoute {
            path: path.to_string(),
            method: method.to_uppercase(),
            flow_id: flow_id.to_string(),
        }])
    }
}

#[async_trait]
impl Trigger for HttpTrigger {
    fn trigger_type(&self) -> &str {
        "http"
    }

    fn description(&self) -> &str {
        "HTTP webhook trigger"
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "URL path for the webhook"
                },
                "method": {
                    "type": "string",
                    "default": "POST",
                    "description": "HTTP method"
                },
                "flow_id": {
                    "type": "string",
                    "description": "Flow to trigger"
                }
            },
            "required": ["path", "flow_id"]
        })
    }

    fn validate_config(&self, config: &Value) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        match config.get("path").and_then(|v| v.as_str()) {
            None => errors.push("missing 'path'".into()),
            Some(p) if !p.starts_with('/') => errors.push(format!("path must start with '/': {p}")),
            _ => {}
        }

        if config.get("flow_id").and_then(|v| v.as_str()).is_none() {
            errors.push("missing 'flow_id'".into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Awaits the shutdown signal. Actual HTTP handling is done by the API layer.
    async fn start(
        &self,
        _config: Value,
        _tx: mpsc::Sender<TriggerEvent>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), TriggerError> {
        let _ = shutdown.recv().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routes_from_config() {
        let trigger = HttpTrigger;
        let config = json!({
            "path": "/webhook/enrich",
            "method": "POST",
            "flow_id": "enrich-flow"
        });

        let routes = trigger.routes(&config).expect("should parse routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/webhook/enrich");
        assert_eq!(routes[0].method, "POST");
        assert_eq!(routes[0].flow_id, "enrich-flow");
    }

    #[test]
    fn test_routes_default_method() {
        let trigger = HttpTrigger;
        let config = json!({
            "path": "/hook",
            "flow_id": "my-flow"
        });

        let routes = trigger.routes(&config).expect("should parse routes");
        assert_eq!(routes[0].method, "POST");
    }

    #[test]
    fn test_validate_config_valid() {
        let trigger = HttpTrigger;
        let config = json!({
            "path": "/webhook/test",
            "flow_id": "test-flow"
        });
        assert!(trigger.validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_config_invalid_path() {
        let trigger = HttpTrigger;
        let config = json!({
            "path": "no-leading-slash",
            "flow_id": "test-flow"
        });
        let err = trigger.validate_config(&config).unwrap_err();
        assert!(err[0].contains("must start with '/'"));
    }

    #[test]
    fn test_validate_config_missing_path() {
        let trigger = HttpTrigger;
        let config = json!({"flow_id": "test"});
        let err = trigger.validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("missing 'path'")));
    }

    #[test]
    fn test_validate_config_missing_flow_id() {
        let trigger = HttpTrigger;
        let config = json!({"path": "/hook"});
        let err = trigger.validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("missing 'flow_id'")));
    }

    #[tokio::test]
    async fn test_start_awaits_shutdown() {
        let trigger = HttpTrigger;
        let (tx, _rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let handle = tokio::spawn(async move {
            trigger
                .start(json!({}), tx, shutdown_rx)
                .await
                .expect("should not fail");
        });

        // Give start() a moment to begin waiting.
        tokio::task::yield_now().await;

        // Send shutdown.
        shutdown_tx.send(()).expect("send shutdown");

        // start() should return.
        handle.await.expect("task should complete");
    }
}
