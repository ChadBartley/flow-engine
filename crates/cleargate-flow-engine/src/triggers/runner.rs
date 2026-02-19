//! Trigger lifecycle management.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::super::traits::Trigger;
use super::super::types::TriggerEvent;

/// A configured trigger instance bound to a specific flow.
pub struct TriggerInstance {
    /// The trigger implementation.
    pub trigger: Arc<dyn Trigger>,
    /// Which flow this trigger is bound to.
    pub flow_id: String,
    /// Trigger-specific configuration.
    pub config: Value,
}

/// Manages the lifecycle of all trigger instances.
///
/// Each trigger runs in its own tokio task. The runner provides a unified
/// start/shutdown interface.
pub struct TriggerRunner {
    instances: Vec<TriggerInstance>,
    event_tx: mpsc::Sender<TriggerEvent>,
    shutdown_tx: broadcast::Sender<()>,
}

impl TriggerRunner {
    /// Create a new trigger runner.
    ///
    /// - `instances` — configured triggers to manage
    /// - `event_tx` — channel for triggers to send events on
    /// - `shutdown_tx` — broadcast channel to signal all triggers to stop
    pub fn new(
        instances: Vec<TriggerInstance>,
        event_tx: mpsc::Sender<TriggerEvent>,
        shutdown_tx: broadcast::Sender<()>,
    ) -> Self {
        Self {
            instances,
            event_tx,
            shutdown_tx,
        }
    }

    /// Spawn a tokio task for each trigger instance.
    ///
    /// Returns the `JoinHandle`s so the caller can await them.
    pub fn start(&self) -> Vec<JoinHandle<()>> {
        self.instances
            .iter()
            .map(|instance| {
                let trigger = Arc::clone(&instance.trigger);
                let config = instance.config.clone();
                let tx = self.event_tx.clone();
                let shutdown_rx = self.shutdown_tx.subscribe();
                let flow_id = instance.flow_id.clone();
                let trigger_type = trigger.trigger_type().to_string();

                tokio::spawn(async move {
                    if let Err(e) = trigger.start(config, tx, shutdown_rx).await {
                        tracing::error!(
                            flow_id = %flow_id,
                            trigger_type = %trigger_type,
                            "trigger failed: {e}"
                        );
                    }
                })
            })
            .collect()
    }

    /// Send the shutdown signal and await all trigger tasks.
    pub async fn shutdown(self, handles: Vec<JoinHandle<()>>) {
        // Ignore send error — receivers may already be dropped.
        let _ = self.shutdown_tx.send(());

        for handle in handles {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::TriggerError;
    use async_trait::async_trait;
    use serde_json::json;

    /// A mock trigger that sends one event then awaits shutdown.
    struct MockTrigger {
        flow_id: String,
    }

    #[async_trait]
    impl Trigger for MockTrigger {
        fn trigger_type(&self) -> &str {
            "mock"
        }
        fn description(&self) -> &str {
            "Mock trigger for testing"
        }
        fn config_schema(&self) -> Value {
            json!({})
        }

        async fn start(
            &self,
            _config: Value,
            tx: mpsc::Sender<TriggerEvent>,
            mut shutdown: broadcast::Receiver<()>,
        ) -> Result<(), TriggerError> {
            let event = TriggerEvent {
                flow_id: self.flow_id.clone(),
                version_id: None,
                inputs: json!({"greeting": "hello"}),
                source: crate::types::TriggerSource::Manual {
                    principal: "test".into(),
                },
                idempotency_key: None,
            };
            let _ = tx.send(event).await;
            let _ = shutdown.recv().await;
            Ok(())
        }
    }

    /// A mock trigger whose start() returns an error.
    struct FailingTrigger;

    #[async_trait]
    impl Trigger for FailingTrigger {
        fn trigger_type(&self) -> &str {
            "failing"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn config_schema(&self) -> Value {
            json!({})
        }

        async fn start(
            &self,
            _config: Value,
            _tx: mpsc::Sender<TriggerEvent>,
            _shutdown: broadcast::Receiver<()>,
        ) -> Result<(), TriggerError> {
            Err(TriggerError::Runtime {
                message: "intentional failure".into(),
            })
        }
    }

    #[tokio::test]
    async fn test_start_and_shutdown() {
        let (event_tx, mut event_rx) = mpsc::channel(100);
        let (shutdown_tx, _) = broadcast::channel(1);

        let instances = vec![TriggerInstance {
            trigger: Arc::new(MockTrigger {
                flow_id: "flow-1".into(),
            }),
            flow_id: "flow-1".into(),
            config: json!({}),
        }];

        let runner = TriggerRunner::new(instances, event_tx, shutdown_tx);
        let handles = runner.start();
        assert_eq!(handles.len(), 1);

        // Receive the event the mock trigger sends.
        let event = event_rx.recv().await.expect("should receive event");
        assert_eq!(event.flow_id, "flow-1");

        // Shutdown cleanly.
        runner.shutdown(handles).await;
    }

    #[tokio::test]
    async fn test_trigger_error_logged() {
        let (event_tx, _event_rx) = mpsc::channel(100);
        let (shutdown_tx, _) = broadcast::channel(1);

        let instances = vec![TriggerInstance {
            trigger: Arc::new(FailingTrigger),
            flow_id: "flow-err".into(),
            config: json!({}),
        }];

        let runner = TriggerRunner::new(instances, event_tx, shutdown_tx);
        let handles = runner.start();

        // The task should complete without panic (error is logged).
        runner.shutdown(handles).await;
    }
}
