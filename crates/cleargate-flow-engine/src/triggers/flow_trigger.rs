//! Flow completion trigger.
//!
//! Fires when another flow's run completes, enabling flow chaining.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use super::super::errors::TriggerError;
use super::super::traits::Trigger;
use super::super::types::{RunStatus, TriggerEvent, TriggerSource};

/// Broadcast by the executor when a run finishes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunCompletedEvent {
    /// The run that completed.
    pub run_id: String,
    /// The flow the run belongs to.
    pub flow_id: String,
    /// Terminal status of the run.
    pub status: RunStatus,
    /// Final outputs (or empty object).
    pub outputs: Value,
}

/// Flow completion trigger.
///
/// Subscribes to run completion broadcasts and emits a [`TriggerEvent`]
/// when a matching flow completes with the expected status.
///
/// Holds a `broadcast::Sender` and subscribes in [`start()`](Trigger::start)
/// so the `Trigger` trait's `&self` signature is satisfied.
pub struct FlowTrigger {
    run_completed_tx: broadcast::Sender<RunCompletedEvent>,
}

impl FlowTrigger {
    /// Create a new flow trigger.
    ///
    /// Pass the sending half of the run-completed broadcast channel. The
    /// executor (M7) owns a clone and sends events when runs finish.
    /// `start()` subscribes a fresh receiver from this sender.
    pub fn new(run_completed_tx: broadcast::Sender<RunCompletedEvent>) -> Self {
        Self { run_completed_tx }
    }
}

/// Parse the config to extract source_flow_id and status filter.
fn parse_flow_config(config: &Value) -> Result<(String, String, Vec<RunStatus>), TriggerError> {
    let source_flow_id = config
        .get("source_flow_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TriggerError::Config {
            message: "missing 'source_flow_id' in config".into(),
        })?
        .to_string();

    let flow_id = config
        .get("flow_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TriggerError::Config {
            message: "missing 'flow_id' in config".into(),
        })?
        .to_string();

    let status_filter = if let Some(arr) = config.get("status_filter").and_then(|v| v.as_array()) {
        arr.iter()
            .filter_map(|v| v.as_str())
            .filter_map(|s| serde_json::from_value(Value::String(s.to_string())).ok())
            .collect()
    } else {
        vec![RunStatus::Completed]
    };

    Ok((source_flow_id, flow_id, status_filter))
}

#[async_trait]
impl Trigger for FlowTrigger {
    fn trigger_type(&self) -> &str {
        "flow"
    }

    fn description(&self) -> &str {
        "Flow completion trigger"
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_flow_id": {
                    "type": "string",
                    "description": "Flow ID to watch for completions"
                },
                "status_filter": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Run statuses that trigger (default: [\"completed\"])"
                },
                "flow_id": {
                    "type": "string",
                    "description": "Flow to trigger on match"
                }
            },
            "required": ["source_flow_id", "flow_id"]
        })
    }

    fn validate_config(&self, config: &Value) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if config
            .get("source_flow_id")
            .and_then(|v| v.as_str())
            .is_none()
        {
            errors.push("missing 'source_flow_id'".into());
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

    async fn start(
        &self,
        config: Value,
        tx: mpsc::Sender<TriggerEvent>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), TriggerError> {
        let (source_flow_id, flow_id, status_filter) = parse_flow_config(&config)?;
        let mut run_rx = self.run_completed_tx.subscribe();

        loop {
            tokio::select! {
                result = run_rx.recv() => {
                    let completed = match result {
                        Ok(evt) => evt,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                lagged = n,
                                "flow trigger missed {n} run completion events"
                            );
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Ok(());
                        }
                    };

                    // Filter by source flow and status.
                    if completed.flow_id != source_flow_id {
                        continue;
                    }
                    if !status_filter.contains(&completed.status) {
                        continue;
                    }

                    let event = TriggerEvent {
                        flow_id: flow_id.clone(),
                        version_id: None,
                        inputs: completed.outputs,
                        source: TriggerSource::Flow {
                            source_run_id: completed.run_id,
                            source_flow_id: completed.flow_id,
                        },
                        idempotency_key: None,
                    };

                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
                _ = shutdown.recv() => {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_completed_event(flow_id: &str, status: RunStatus) -> RunCompletedEvent {
        RunCompletedEvent {
            run_id: "run-1".into(),
            flow_id: flow_id.into(),
            status,
            outputs: json!({"result": "done"}),
        }
    }

    #[tokio::test]
    async fn test_flow_trigger_fires_on_match() {
        let (run_tx, _) = broadcast::channel(10);
        let trigger = FlowTrigger::new(run_tx.clone());

        let (event_tx, mut event_rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let config = json!({
            "source_flow_id": "upstream-flow",
            "flow_id": "downstream-flow"
        });

        let handle =
            tokio::spawn(async move { trigger.start(config, event_tx, shutdown_rx).await });

        // Yield so the spawned task subscribes to the broadcast channel.
        tokio::task::yield_now().await;

        // Broadcast a matching run completion.
        run_tx
            .send(make_completed_event("upstream-flow", RunStatus::Completed))
            .expect("send");

        let event = event_rx.recv().await.expect("should receive event");
        assert_eq!(event.flow_id, "downstream-flow");
        if let TriggerSource::Flow { source_flow_id, .. } = &event.source {
            assert_eq!(source_flow_id, "upstream-flow");
        } else {
            panic!("expected TriggerSource::Flow");
        }

        shutdown_tx.send(()).expect("shutdown");
        handle.await.expect("task completes").expect("no error");
    }

    #[tokio::test]
    async fn test_flow_trigger_ignores_non_matching() {
        let (run_tx, _) = broadcast::channel(10);
        let trigger = FlowTrigger::new(run_tx.clone());

        let (event_tx, mut event_rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let config = json!({
            "source_flow_id": "upstream-flow",
            "flow_id": "downstream-flow"
        });

        let handle =
            tokio::spawn(async move { trigger.start(config, event_tx, shutdown_rx).await });

        // Yield so the spawned task subscribes to the broadcast channel.
        tokio::task::yield_now().await;

        // Broadcast events for different flow IDs — should not trigger.
        run_tx
            .send(make_completed_event("other-flow", RunStatus::Completed))
            .expect("send");
        run_tx
            .send(make_completed_event("another-flow", RunStatus::Completed))
            .expect("send");

        // Give events a chance to be processed.
        tokio::task::yield_now().await;

        // No event should arrive.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), event_rx.recv()).await;
        assert!(result.is_err(), "should timeout — no events expected");

        shutdown_tx.send(()).expect("shutdown");
        handle.await.expect("task completes").expect("no error");
    }

    #[tokio::test]
    async fn test_flow_trigger_status_filter() {
        let (run_tx, _) = broadcast::channel(10);
        let trigger = FlowTrigger::new(run_tx.clone());

        let (event_tx, mut event_rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let config = json!({
            "source_flow_id": "upstream-flow",
            "flow_id": "downstream-flow",
            "status_filter": ["failed"]
        });

        let handle =
            tokio::spawn(async move { trigger.start(config, event_tx, shutdown_rx).await });

        // Yield so the spawned task subscribes to the broadcast channel.
        tokio::task::yield_now().await;

        // Completed status should NOT trigger (filter is ["failed"]).
        run_tx
            .send(make_completed_event("upstream-flow", RunStatus::Completed))
            .expect("send");

        tokio::task::yield_now().await;

        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), event_rx.recv()).await;
        assert!(
            result.is_err(),
            "completed status should not match failed filter"
        );

        // Failed status SHOULD trigger.
        run_tx
            .send(make_completed_event("upstream-flow", RunStatus::Failed))
            .expect("send");

        let event = event_rx.recv().await.expect("should receive event");
        assert_eq!(event.flow_id, "downstream-flow");

        shutdown_tx.send(()).expect("shutdown");
        handle.await.expect("task completes").expect("no error");
    }
}
