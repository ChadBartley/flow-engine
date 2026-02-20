//! Built-in node for graph composition via sub-flow execution.
//!
//! The [`SubflowNode`] embeds one [`GraphDef`] as a single node inside
//! another graph, enabling reusable flow components. The sub-flow runs
//! as a child execution with its own run ID (parent-child relationship
//! tracked in the RunStore).
//!
//! ## Resolution modes
//!
//! - **Named**: `{ "subflow": "my-reusable-flow" }` — resolves from the
//!   [`SubflowRegistry`](crate::subflow_registry::SubflowRegistry).
//! - **Inline**: `{ "subflow_inline": { ... } }` — the [`GraphDef`] is
//!   embedded directly in the node config.
//!
//! ## Input/output mapping
//!
//! - Parent node inputs are passed as the sub-flow's trigger inputs.
//! - The sub-flow's terminal node outputs become this node's output.
//! - Optional `input_mapping` / `output_mapping` in config for field renaming.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;
use crate::write_event::WriteEvent;

/// Built-in node that executes a sub-flow graph as a single node.
///
/// Register named sub-flows via
/// [`EngineBuilder::register_subflow`](crate::EngineBuilder::register_subflow),
/// or embed a [`GraphDef`] inline in the node config.
pub struct SubflowNode;

/// Apply an input or output mapping: rename fields according to
/// a `{ "source_key": "target_key" }` map.
fn apply_mapping(value: &Value, mapping: &Value) -> Value {
    let Some(map) = mapping.as_object() else {
        return value.clone();
    };
    let Some(obj) = value.as_object() else {
        return value.clone();
    };

    let mut result = obj.clone();
    for (from, to) in map {
        if let Some(to_key) = to.as_str() {
            if let Some(val) = result.remove(from) {
                result.insert(to_key.to_string(), val);
            }
        }
    }
    Value::Object(result)
}

/// Resolve the `GraphDef` from config — either by name or inline.
fn resolve_graph(config: &Value, ctx: &NodeCtx) -> Result<GraphDef, NodeError> {
    // Named sub-flow: look up in registry.
    if let Some(name) = config.get("subflow").and_then(|v| v.as_str()) {
        let registry = ctx.subflow_registry();
        return registry.get(name).ok_or_else(|| NodeError::Fatal {
            message: format!("sub-flow not found in registry: {name}"),
        });
    }

    // Inline sub-flow: deserialize from config.
    if let Some(inline) = config.get("subflow_inline") {
        let graph: GraphDef =
            serde_json::from_value(inline.clone()).map_err(|e| NodeError::Fatal {
                message: format!("invalid inline sub-flow graph: {e}"),
            })?;
        return Ok(graph);
    }

    Err(NodeError::Validation {
        message: "config must contain either \"subflow\" (name) or \"subflow_inline\" (graph)"
            .into(),
    })
}

#[async_trait]
impl NodeHandler for SubflowNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "subflow".into(),
            label: "Sub-Flow".into(),
            category: "composition".into(),
            inputs: vec![PortDef {
                name: "json".into(),
                port_type: PortType::Json,
                required: true,
                default: None,
                description: Some("Inputs passed to the sub-flow's entry nodes".into()),
                sensitivity: Sensitivity::None,
            }],
            outputs: vec![PortDef {
                name: "json".into(),
                port_type: PortType::Json,
                required: true,
                default: None,
                description: Some("Outputs from the sub-flow's terminal nodes".into()),
                sensitivity: Sensitivity::None,
            }],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "subflow": {
                        "type": "string",
                        "description": "Name of a registered sub-flow"
                    },
                    "subflow_inline": {
                        "type": "object",
                        "description": "Inline GraphDef to execute"
                    },
                    "input_mapping": {
                        "type": "object",
                        "description": "Rename input fields: { source_key: target_key }"
                    },
                    "output_mapping": {
                        "type": "object",
                        "description": "Rename output fields: { source_key: target_key }"
                    }
                }
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        // 1. Resolve the graph definition.
        let graph = resolve_graph(config, ctx)?;

        // 2. Validate graph has nodes.
        if graph.nodes.is_empty() {
            return Err(NodeError::Validation {
                message: "sub-flow graph contains no nodes".into(),
            });
        }

        // 3. Apply input mapping if configured.
        let mapped_inputs = if let Some(mapping) = config.get("input_mapping") {
            apply_mapping(&inputs, mapping)
        } else {
            inputs
        };

        // 4. Execute the sub-flow as a child run.
        let executor = ctx.executor().ok_or_else(|| NodeError::Fatal {
            message: "executor reference not available — sub-flow execution requires \
                      a fully constructed Engine"
                .into(),
        })?;
        let trigger_source = TriggerSource::Flow {
            source_run_id: ctx.run_id().to_string(),
            source_flow_id: graph.id.clone(),
        };

        let version_id = format!("subflow:{}", graph.id);
        let handle = executor
            .execute(
                &graph,
                &version_id,
                mapped_inputs,
                trigger_source,
                Some(ctx.run_id()),
            )
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("sub-flow execution failed to start: {e}"),
            })?;

        ctx.emit(
            "subflow.started",
            json!({
                "child_run_id": handle.run_id,
                "subflow_id": graph.id,
            }),
        );

        // 5. Consume events until completion, collecting terminal outputs.
        let mut events = handle.events;
        let mut terminal_outputs: HashMap<String, Value> = HashMap::new();
        let mut child_status = RunStatus::Completed;

        loop {
            match events.recv().await {
                Ok(event) => match &event {
                    WriteEvent::NodeCompleted {
                        node_id, outputs, ..
                    } => {
                        terminal_outputs.insert(node_id.clone(), outputs.clone());
                    }
                    WriteEvent::RunCompleted { status, .. } => {
                        child_status = status.clone();
                        break;
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        lagged = n,
                        "sub-flow event receiver lagged, some events skipped"
                    );
                }
            }
        }

        // 6. Check child status.
        match child_status {
            RunStatus::Completed => {}
            RunStatus::Failed => {
                return Err(NodeError::Fatal {
                    message: "sub-flow execution failed".into(),
                });
            }
            RunStatus::Cancelled => {
                return Err(NodeError::Fatal {
                    message: "sub-flow execution was cancelled".into(),
                });
            }
            other => {
                return Err(NodeError::Fatal {
                    message: format!("sub-flow ended with unexpected status: {other:?}"),
                });
            }
        }

        // 7. Build output from terminal node outputs.
        // If there's exactly one terminal node, use its output directly.
        // Otherwise, wrap all outputs in a map keyed by node ID.
        let raw_output = if terminal_outputs.len() == 1 {
            terminal_outputs.into_values().next().unwrap_or(json!({}))
        } else {
            json!(terminal_outputs)
        };

        // 8. Apply output mapping if configured.
        let output = if let Some(mapping) = config.get("output_mapping") {
            apply_mapping(&raw_output, mapping)
        } else {
            raw_output
        };

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_mapping_renames_fields() {
        let input = json!({"foo": 1, "bar": 2, "baz": 3});
        let mapping = json!({"foo": "renamed_foo", "bar": "renamed_bar"});
        let result = apply_mapping(&input, &mapping);
        assert_eq!(
            result,
            json!({"renamed_foo": 1, "renamed_bar": 2, "baz": 3})
        );
    }

    #[test]
    fn test_apply_mapping_no_mapping_returns_clone() {
        let input = json!({"a": 1});
        let result = apply_mapping(&input, &json!(null));
        assert_eq!(result, input);
    }

    #[test]
    fn test_apply_mapping_non_object_input() {
        let input = json!("scalar");
        let mapping = json!({"a": "b"});
        let result = apply_mapping(&input, &mapping);
        assert_eq!(result, json!("scalar"));
    }

    #[test]
    fn test_meta() {
        let node = SubflowNode;
        let meta = node.meta();
        assert_eq!(meta.node_type, "subflow");
        assert_eq!(meta.category, "composition");
    }
}
