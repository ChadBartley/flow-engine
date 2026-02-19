//! Built-in HTTP Call node handler.
//!
//! This node makes HTTP requests to external services within a flow.
//! Supports GET, POST, PUT, PATCH, DELETE methods with optional
//! authentication via secrets and custom headers.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in node that makes HTTP requests to external services.
pub struct HttpNode;

#[async_trait]
impl NodeHandler for HttpNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "http_call".into(),
            label: "HTTP Call".into(),
            category: "integration".into(),
            inputs: vec![PortDef {
                name: "body".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: false,
                default: None,
                description: None,
            }],
            outputs: vec![PortDef {
                name: "response".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: None,
            }],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "method": { "type": "string" },
                    "headers": { "type": "object" },
                    "auth_secret": { "type": "string" }
                },
                "required": ["url"]
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        // 1. Read required "url" from config
        let url = config
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required config field: url".into(),
            })?
            .to_string();

        // 2. Read optional "method" (default: GET), convert to uppercase
        let method_str = config
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        // 3. Read optional "headers" (object of string -> string)
        let custom_headers: Vec<(String, String)> = config
            .get("headers")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // 4. If "auth_secret" is set, read the secret and prepare Bearer auth
        let auth_header =
            if let Some(secret_name) = config.get("auth_secret").and_then(|v| v.as_str()) {
                let key = ctx.secret(secret_name).await?;
                Some(format!("Bearer {key}"))
            } else {
                None
            };

        // 5. Build request
        let method =
            reqwest::Method::from_bytes(method_str.as_bytes()).unwrap_or(reqwest::Method::GET);

        let mut request_builder = ctx.http().request(method.clone(), &url);

        // Set custom headers
        for (key, value) in &custom_headers {
            request_builder = request_builder.header(key.as_str(), value.as_str());
        }

        // Set auth header
        if let Some(ref auth) = auth_header {
            request_builder = request_builder.header("Authorization", auth.as_str());
        }

        // For POST/PUT/PATCH, set inputs as JSON body
        if method == reqwest::Method::POST
            || method == reqwest::Method::PUT
            || method == reqwest::Method::PATCH
        {
            request_builder = request_builder.json(&inputs);
        }

        // 6. Send request, get response
        let response = request_builder
            .send()
            .await
            .map_err(|e| NodeError::Retryable {
                message: format!("HTTP request failed: {e}"),
            })?;

        let status = response.status();

        // 7. Handle response based on status code
        if status.is_success() {
            // Try to parse body as JSON; if that fails, return body text as Value::String
            let body_text = response.text().await.map_err(|e| NodeError::Retryable {
                message: format!("failed to read response body: {e}"),
            })?;

            let body_value: Value =
                serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));

            Ok(json!({ "response": body_value }))
        } else if status.is_server_error() {
            // 8. 5xx: retryable
            Err(NodeError::Retryable {
                message: format!("HTTP {status}"),
            })
        } else {
            // 9. 4xx or other: fatal
            Err(NodeError::Fatal {
                message: format!("HTTP {status}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_http_get() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": 1})))
            .mount(&server)
            .await;

        let node = HttpNode;
        let config = json!({
            "url": format!("{}/data", server.uri()),
        });
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let result = node.run(json!({}), &config, &ctx).await.unwrap();
        assert_eq!(result["response"], json!({"result": 1}));
    }

    #[tokio::test]
    async fn test_http_post_with_body() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/submit"))
            .and(body_json(json!({"x": 1})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let node = HttpNode;
        let config = json!({
            "url": format!("{}/submit", server.uri()),
            "method": "POST",
        });
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let result = node.run(json!({"x": 1}), &config, &ctx).await.unwrap();
        assert_eq!(result["response"], json!({"ok": true}));
    }

    #[tokio::test]
    async fn test_http_auth_header() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/secure"))
            .and(header("Authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"authed": true})))
            .mount(&server)
            .await;

        let node = HttpNode;
        let config = json!({
            "url": format!("{}/secure", server.uri()),
            "auth_secret": "MY_KEY",
        });
        let (ctx, _inspector) = TestNodeCtx::builder().secret("MY_KEY", "sk-test").build();

        let result = node.run(json!({}), &config, &ctx).await.unwrap();
        assert_eq!(result["response"], json!({"authed": true}));
    }

    #[tokio::test]
    async fn test_http_server_error_retryable() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/fail"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let node = HttpNode;
        let config = json!({
            "url": format!("{}/fail", server.uri()),
        });
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let err = node.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Retryable { message } => {
                assert!(message.contains("500"), "got: {message}");
            }
            other => panic!("expected Retryable, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_http_client_error_fatal() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/bad"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;

        let node = HttpNode;
        let config = json!({
            "url": format!("{}/bad", server.uri()),
        });
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let err = node.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("400"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }
}
