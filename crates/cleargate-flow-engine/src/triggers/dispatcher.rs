//! Trigger event dispatcher.
//!
//! Receives [`TriggerEvent`]s, resolves the target flow version, deduplicates
//! by idempotency key, and collects dispatched runs. Actual execution is
//! handled by the engine (M7).

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;

use super::super::types::{GraphDef, TriggerEvent, TriggerSource};
use super::super::versioning::FlowVersioning;

/// A resolved trigger dispatch ready for execution.
#[derive(Debug, Clone)]
pub struct DispatchedRun {
    /// The target flow.
    pub flow_id: String,
    /// Content-addressed version of the flow graph.
    pub version_id: String,
    /// The resolved graph snapshot.
    pub graph: GraphDef,
    /// Inputs from the trigger event.
    pub inputs: Value,
    /// How the run was triggered.
    pub source: TriggerSource,
}

/// Receives trigger events, resolves flow versions, and collects dispatches.
///
/// The dispatcher does NOT call `engine.execute()` — it resolves the flow
/// and version, deduplicates by idempotency key, and returns the collected
/// dispatches when the event channel closes. Integration with the executor
/// happens in M7/M10.
pub struct TriggerDispatcher {
    event_rx: mpsc::Receiver<TriggerEvent>,
    versioning: Arc<FlowVersioning>,
}

impl TriggerDispatcher {
    /// Create a new dispatcher.
    ///
    /// - `event_rx` — channel receiving trigger events
    /// - `versioning` — flow versioning service for resolving graphs
    pub fn new(event_rx: mpsc::Receiver<TriggerEvent>, versioning: Arc<FlowVersioning>) -> Self {
        Self {
            event_rx,
            versioning,
        }
    }

    /// Process trigger events until the channel closes.
    ///
    /// Returns all successfully dispatched runs. Events with duplicate
    /// idempotency keys or unresolvable flows are skipped.
    pub async fn run(mut self) -> Vec<DispatchedRun> {
        let mut dispatched = Vec::new();
        let mut seen_keys: HashSet<String> = HashSet::new();

        while let Some(event) = self.event_rx.recv().await {
            // Idempotency check.
            if let Some(ref key) = event.idempotency_key {
                if !seen_keys.insert(key.clone()) {
                    tracing::debug!(key = %key, "skipping duplicate trigger event");
                    continue;
                }
            }

            // Resolve the flow version.
            let version = if let Some(ref vid) = event.version_id {
                match self.versioning.get_at_version(vid).await {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        tracing::warn!(
                            version_id = %vid,
                            flow_id = %event.flow_id,
                            "trigger event references unknown version, skipping"
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            flow_id = %event.flow_id,
                            "failed to resolve flow version, skipping"
                        );
                        continue;
                    }
                }
            } else {
                match self.versioning.get_current(&event.flow_id).await {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        tracing::warn!(
                            flow_id = %event.flow_id,
                            "no current version for flow, skipping"
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            flow_id = %event.flow_id,
                            "failed to resolve current flow version, skipping"
                        );
                        continue;
                    }
                }
            };

            dispatched.push(DispatchedRun {
                flow_id: event.flow_id,
                version_id: version.version_id.clone(),
                graph: version.graph,
                inputs: event.inputs,
                source: event.source,
            });
        }

        dispatched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::FileFlowStore;
    use crate::types::{GraphDef, TriggerSource};
    use serde_json::json;
    use std::collections::BTreeMap;

    /// Create a minimal graph for testing.
    fn make_graph(id: &str) -> GraphDef {
        GraphDef {
            schema_version: 1,
            id: id.into(),
            name: format!("Test {id}"),
            version: "1.0".into(),
            nodes: vec![],
            edges: vec![],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        }
    }

    /// Set up a FlowVersioning with a saved flow, returning the versioning
    /// service and the version_id.
    async fn setup_versioning_with_flow(flow_id: &str) -> (Arc<FlowVersioning>, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileFlowStore::new(dir.path().to_path_buf()).expect("create store"));
        let versioning = Arc::new(FlowVersioning::new(store));

        let graph = make_graph(flow_id);
        let version = versioning
            .save(flow_id, &format!("Test {flow_id}"), graph, None, None)
            .await
            .expect("save");

        // Keep tempdir alive by leaking it (test only).
        std::mem::forget(dir);

        (versioning, version.version_id)
    }

    #[tokio::test]
    async fn test_dispatch_resolves_version() {
        let (versioning, version_id) = setup_versioning_with_flow("my-flow").await;
        let (tx, rx) = mpsc::channel(10);

        let dispatcher = TriggerDispatcher::new(rx, versioning);

        tx.send(TriggerEvent {
            flow_id: "my-flow".into(),
            version_id: None,
            inputs: json!({"key": "value"}),
            source: TriggerSource::Manual {
                principal: "test".into(),
            },
            idempotency_key: None,
        })
        .await
        .expect("send");

        // Close the channel so run() terminates.
        drop(tx);

        let dispatched = dispatcher.run().await;
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].flow_id, "my-flow");
        assert_eq!(dispatched[0].version_id, version_id);
        assert_eq!(dispatched[0].inputs, json!({"key": "value"}));
    }

    #[tokio::test]
    async fn test_idempotency_dedup() {
        let (versioning, _) = setup_versioning_with_flow("dedup-flow").await;
        let (tx, rx) = mpsc::channel(10);

        let dispatcher = TriggerDispatcher::new(rx, versioning);

        let event = TriggerEvent {
            flow_id: "dedup-flow".into(),
            version_id: None,
            inputs: json!({}),
            source: TriggerSource::Manual {
                principal: "test".into(),
            },
            idempotency_key: Some("unique-key-1".into()),
        };

        // Send the same idempotency key twice.
        tx.send(event.clone()).await.expect("send 1");
        tx.send(event).await.expect("send 2");
        drop(tx);

        let dispatched = dispatcher.run().await;
        assert_eq!(dispatched.len(), 1, "duplicate should be skipped");
    }

    #[tokio::test]
    async fn test_missing_flow_skipped() {
        let (versioning, _) = setup_versioning_with_flow("existing-flow").await;
        let (tx, rx) = mpsc::channel(10);

        let dispatcher = TriggerDispatcher::new(rx, versioning);

        // Send event for a flow that doesn't exist.
        tx.send(TriggerEvent {
            flow_id: "nonexistent-flow".into(),
            version_id: None,
            inputs: json!({}),
            source: TriggerSource::Manual {
                principal: "test".into(),
            },
            idempotency_key: None,
        })
        .await
        .expect("send");

        drop(tx);

        let dispatched = dispatcher.run().await;
        assert!(dispatched.is_empty(), "nonexistent flow should be skipped");
    }
}
