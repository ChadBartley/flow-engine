//! Human-in-the-loop node handler for FlowEngine v2.
//!
//! This node pauses execution and waits for external human input
//! delivered through a oneshot channel. The executor's `provide_input()`
//! method completes the channel, unblocking the waiting node.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in node that awaits external human input before proceeding.
///
/// When executed, the node registers a oneshot channel in the human input
/// registry, emits an advisory event, and blocks until a value arrives
/// through the channel (or the sender is dropped, signaling cancellation).
pub struct HumanInLoopNode;

#[async_trait]
impl NodeHandler for HumanInLoopNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "human_in_loop".into(),
            label: "Human Input".into(),
            category: "interaction".into(),
            inputs: vec![PortDef {
                name: "prompt".into(),
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
            config_schema: json!({}),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, _inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        ctx.register_human_input(tx).await?;

        ctx.emit("human_input_requested", config.clone());

        match rx.await {
            Ok(value) => Ok(value),
            Err(_) => Err(NodeError::Fatal {
                message: "Human input canceled".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::node_ctx::{HumanInputRegistry, TestNodeCtx};

    #[tokio::test]
    async fn test_human_in_loop_receives_input() {
        let registry: HumanInputRegistry = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let (ctx, _inspector) = TestNodeCtx::builder()
            .run_id("r1")
            .node_id("n1")
            .with_human_input_registry(registry.clone())
            .build();

        let node = HumanInLoopNode;
        let config = json!({});

        let handle = tokio::spawn(async move { node.run(json!({}), &config, &ctx).await });

        // Yield so the spawned task gets a chance to register before we extract
        tokio::task::yield_now().await;

        let tx = {
            let mut guard = registry.lock().await;
            guard
                .remove(&("r1".to_string(), "n1".to_string()))
                .expect("sender should be registered")
        };

        tx.send(json!({"answer": 42})).expect("send should succeed");

        let result = handle.await.expect("task should not panic");
        assert_eq!(result.unwrap(), json!({"answer": 42}));
    }

    #[tokio::test]
    async fn test_human_in_loop_canceled() {
        let registry: HumanInputRegistry = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let (ctx, _inspector) = TestNodeCtx::builder()
            .run_id("r1")
            .node_id("n1")
            .with_human_input_registry(registry.clone())
            .build();

        let node = HumanInLoopNode;
        let config = json!({});

        let handle = tokio::spawn(async move { node.run(json!({}), &config, &ctx).await });

        // Yield so the spawned task gets a chance to register before we drop
        tokio::task::yield_now().await;

        // Drop the sender to simulate cancellation
        {
            let mut guard = registry.lock().await;
            let tx = guard
                .remove(&("r1".to_string(), "n1".to_string()))
                .expect("sender should be registered");
            drop(tx);
        }

        let result = handle.await.expect("task should not panic");
        match result {
            Err(NodeError::Fatal { message }) => {
                assert_eq!(message, "Human input canceled");
            }
            other => panic!("expected Fatal error, got: {other:?}"),
        }
    }
}
