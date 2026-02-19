//! Built-in `tool_router` node handler.
//!
//! Routes LLM tool calls to downstream nodes. Accepts an array of tool
//! calls (either as a `tool_calls` key in an object, or as a bare array)
//! and forwards them unchanged. When the input is missing or empty, the
//! node returns an empty array so downstream fan-out simply does nothing.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Passes an array of LLM tool calls through for downstream fan-out.
pub struct ToolRouterNode;

#[async_trait]
impl NodeHandler for ToolRouterNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "tool_router".into(),
            label: "Tool Router".into(),
            category: "routing".into(),
            inputs: vec![PortDef {
                name: "tool_calls".into(),
                port_type: PortType::List(Box::new(PortType::Json)),
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: None,
            }],
            outputs: vec![PortDef {
                name: "tool_call".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: None,
            }],
            config_schema: json!({}),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(
        &self,
        inputs: Value,
        _config: &Value,
        _ctx: &NodeCtx,
    ) -> Result<Value, NodeError> {
        // Accept either { "tool_calls": [...] } or a bare array.
        let tool_calls = match &inputs {
            Value::Object(map) => map
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default(),
            Value::Array(arr) => arr.clone(),
            _ => Vec::new(),
        };

        Ok(Value::Array(tool_calls))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use serde_json::json;

    #[tokio::test]
    async fn test_tool_router_with_calls() {
        let node = ToolRouterNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "tool_calls": [
                {"id": "1", "tool_name": "a", "arguments": {}},
                {"id": "2", "tool_name": "b", "arguments": {}},
                {"id": "3", "tool_name": "c", "arguments": {}}
            ]
        });

        let result = node.run(inputs, &json!({}), &ctx).await.unwrap();
        let arr = result.as_array().expect("expected array");
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn test_tool_router_empty() {
        let node = ToolRouterNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({"tool_calls": []});

        let result = node.run(inputs, &json!({}), &ctx).await.unwrap();
        let arr = result.as_array().expect("expected array");
        assert!(arr.is_empty());
    }

    #[tokio::test]
    async fn test_tool_router_missing_field() {
        let node = ToolRouterNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({"other": 1});

        let result = node.run(inputs, &json!({}), &ctx).await.unwrap();
        let arr = result.as_array().expect("expected array");
        assert!(arr.is_empty());
    }
}
