//! Conditional node — passthrough for edge-condition routing.
//!
//! This node does no transformation. It forwards its inputs as outputs
//! unchanged. The actual routing logic lives on the edges leaving this
//! node: each edge carries an [`EdgeCondition`] expression that the
//! executor evaluates against the output data to decide which downstream
//! path to follow.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// A passthrough node used for edge-condition routing.
///
/// `ConditionalNode` itself performs no computation. It simply returns
/// its inputs as outputs. The branching decision is made by the executor
/// when it evaluates the [`EdgeCondition`] expressions on the outgoing
/// edges.
pub struct ConditionalNode;

#[async_trait]
impl NodeHandler for ConditionalNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "conditional".into(),
            label: "Conditional".into(),
            category: "routing".into(),
            inputs: vec![PortDef {
                name: "data".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: None,
            }],
            outputs: vec![PortDef {
                name: "data".into(),
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
        Ok(inputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::node_ctx::TestNodeCtx;

    #[tokio::test]
    async fn test_conditional_passthrough() {
        let node = ConditionalNode;
        let input = json!({"x": 42, "status": "ok"});
        let config = json!({});
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let output = node.run(input.clone(), &config, &ctx).await.unwrap();
        assert_eq!(output, input);
    }
}
