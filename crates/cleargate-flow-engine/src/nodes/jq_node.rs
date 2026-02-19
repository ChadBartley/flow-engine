//! Built-in JQ Transform node handler.
//!
//! Runs a jq expression against the input JSON for data transformation.
//! Wraps the `jq-rs` crate. Uses `spawn_blocking` because `jq-rs` is a
//! C binding and must not run on tokio runtime threads.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in node that runs a jq expression against input JSON.
pub struct JqNode;

#[async_trait]
impl NodeHandler for JqNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "jq".into(),
            label: "JQ Transform".into(),
            category: "transform".into(),
            inputs: vec![PortDef {
                name: "input".into(),
                port_type: PortType::Json,
                required: true,
                default: None,
                description: Some("JSON input to transform".into()),
                sensitivity: Sensitivity::default(),
            }],
            outputs: vec![PortDef {
                name: "output".into(),
                port_type: PortType::Json,
                required: true,
                default: None,
                description: Some("Transformed JSON output".into()),
                sensitivity: Sensitivity::default(),
            }],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "JQ expression to evaluate"
                    }
                },
                "required": ["expression"]
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, _ctx: &NodeCtx) -> Result<Value, NodeError> {
        let expression = config
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "config.expression is required".into(),
            })?;

        let input_str = serde_json::to_string(&inputs).map_err(|e| NodeError::Fatal {
            message: format!("serialize input: {e}"),
        })?;

        // jq-rs is a C binding — run on a blocking thread.
        let expr = expression.to_string();
        let result = tokio::task::spawn_blocking(move || jq_rs::run(&expr, &input_str))
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("jq task join: {e}"),
            })?
            .map_err(|e| NodeError::Fatal {
                message: format!("jq error: {e}"),
            })?;

        // jq output may contain trailing newline — trim it.
        let trimmed = result.trim();
        serde_json::from_str(trimmed).map_err(|e| NodeError::Fatal {
            message: format!("jq output parse: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use serde_json::json;

    fn ctx() -> (NodeCtx, crate::node_ctx::TestNodeCtxInspector) {
        TestNodeCtx::builder().build()
    }

    #[tokio::test]
    async fn test_jq_identity() {
        let (ctx, _) = ctx();
        let input = json!({"a": 1});
        let config = json!({"expression": "."});

        let result = JqNode.run(input.clone(), &config, &ctx).await.unwrap();
        assert_eq!(result, input);
    }

    #[tokio::test]
    async fn test_jq_field_access() {
        let (ctx, _) = ctx();
        let input = json!({"name": "Alice", "age": 30});
        let config = json!({"expression": ".name"});

        let result = JqNode.run(input, &config, &ctx).await.unwrap();
        assert_eq!(result, json!("Alice"));
    }

    #[tokio::test]
    async fn test_jq_array_map() {
        let (ctx, _) = ctx();
        let input = json!({"items": [{"price": 10, "qty": 2}, {"price": 5, "qty": 3}]});
        let config = json!({"expression": "[.items[] | .price * .qty]"});

        let result = JqNode.run(input, &config, &ctx).await.unwrap();
        assert_eq!(result, json!([20, 15]));
    }

    #[tokio::test]
    async fn test_jq_filter() {
        let (ctx, _) = ctx();
        let input = json!([
            {"name": "Alice", "active": true},
            {"name": "Bob", "active": false},
            {"name": "Charlie", "active": true}
        ]);
        let config = json!({"expression": "[.[] | select(.active)]"});

        let result = JqNode.run(input, &config, &ctx).await.unwrap();
        assert_eq!(
            result,
            json!([
                {"name": "Alice", "active": true},
                {"name": "Charlie", "active": true}
            ])
        );
    }

    #[tokio::test]
    async fn test_jq_object_construction() {
        let (ctx, _) = ctx();
        let input = json!({"items": [{"price": 10}, {"price": 20}, {"price": 30}]});
        let config =
            json!({"expression": "{total: [.items[].price] | add, count: [.items[]] | length}"});

        let result = JqNode.run(input, &config, &ctx).await.unwrap();
        assert_eq!(result, json!({"total": 60, "count": 3}));
    }

    #[tokio::test]
    async fn test_jq_missing_expression() {
        let (ctx, _) = ctx();
        let config = json!({});

        let err = JqNode.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Validation { message } => {
                assert!(message.contains("expression"), "got: {message}");
            }
            other => panic!("expected Validation, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_jq_invalid_expression() {
        let (ctx, _) = ctx();
        let config = json!({"expression": "invalid[[["});

        let err = JqNode
            .run(json!({"a": 1}), &config, &ctx)
            .await
            .unwrap_err();
        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("jq error"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_jq_null_input() {
        let (ctx, _) = ctx();
        let config = json!({"expression": ".foo"});

        let result = JqNode.run(json!({}), &config, &ctx).await.unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn test_jq_node_meta() {
        let meta = JqNode.meta();
        assert_eq!(meta.node_type, "jq");
        assert_eq!(meta.category, "transform");
        assert_eq!(meta.inputs.len(), 1);
        assert_eq!(meta.inputs[0].name, "input");
        assert_eq!(meta.outputs.len(), 1);
        assert_eq!(meta.outputs[0].name, "output");
    }
}
