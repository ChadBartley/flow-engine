//! Cron schedule trigger.
//!
//! Fires on a cron schedule. Uses idempotency keys to prevent duplicate
//! executions for the same scheduled tick.

use async_trait::async_trait;
use chrono::Utc;
use cron::Schedule;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use super::super::errors::TriggerError;
use super::super::traits::Trigger;
use super::super::types::{TriggerEvent, TriggerSource};

/// Cron schedule trigger.
///
/// Emits a [`TriggerEvent`] at each cron tick with an idempotency key of
/// `cron:{flow_id}:{epoch_secs}` to prevent duplicate executions.
pub struct CronTrigger;

/// Convert a 5-field cron expression to the 7-field format the `cron` crate expects.
///
/// Standard cron: `min hour day month weekday`
/// Cron crate:    `sec min hour day month weekday year`
fn normalize_cron_expression(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    match fields.len() {
        5 => format!("0 {expr} *"),
        6 => format!("0 {expr}"),
        _ => expr.to_string(),
    }
}

fn parse_schedule(config: &Value) -> Result<(Schedule, String), TriggerError> {
    let schedule_str = config
        .get("schedule")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TriggerError::Config {
            message: "missing 'schedule' in config".into(),
        })?;

    let flow_id = config
        .get("flow_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TriggerError::Config {
            message: "missing 'flow_id' in config".into(),
        })?;

    let normalized = normalize_cron_expression(schedule_str);
    let schedule: Schedule = normalized.parse().map_err(|e| TriggerError::Config {
        message: format!("invalid cron expression '{schedule_str}': {e}"),
    })?;

    Ok((schedule, flow_id.to_string()))
}

#[async_trait]
impl Trigger for CronTrigger {
    fn trigger_type(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Cron schedule trigger"
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "schedule": {
                    "type": "string",
                    "description": "Cron expression (5 or 7 field)"
                },
                "flow_id": {
                    "type": "string",
                    "description": "Flow to trigger"
                }
            },
            "required": ["schedule", "flow_id"]
        })
    }

    fn validate_config(&self, config: &Value) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        match config.get("schedule").and_then(|v| v.as_str()) {
            None => errors.push("missing 'schedule'".into()),
            Some(s) => {
                let normalized = normalize_cron_expression(s);
                if normalized.parse::<Schedule>().is_err() {
                    errors.push(format!("invalid cron expression: {s}"));
                }
            }
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
        let (schedule, flow_id) = parse_schedule(&config)?;

        loop {
            let now = Utc::now();
            let next = schedule
                .upcoming(Utc)
                .next()
                .ok_or_else(|| TriggerError::Runtime {
                    message: "cron schedule has no upcoming occurrences".into(),
                })?;

            let delay = (next - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_millis(100));

            tokio::select! {
                _ = tokio::time::sleep(delay) => {
                    let tick = Utc::now();
                    let event = TriggerEvent {
                        flow_id: flow_id.clone(),
                        version_id: None,
                        inputs: json!({}),
                        source: TriggerSource::Cron {
                            schedule: config.get("schedule")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            tick,
                        },
                        idempotency_key: Some(format!("cron:{}:{}", flow_id, tick.timestamp())),
                    };

                    if tx.send(event).await.is_err() {
                        // Receiver dropped — stop the trigger.
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

    #[test]
    fn test_normalize_5_field() {
        let result = normalize_cron_expression("*/5 * * * *");
        assert_eq!(result, "0 */5 * * * * *");
    }

    #[test]
    fn test_normalize_7_field_passthrough() {
        let input = "0 */5 * * * * *";
        assert_eq!(normalize_cron_expression(input), input);
    }

    #[test]
    fn test_cron_invalid_schedule() {
        let trigger = CronTrigger;
        let config = json!({
            "schedule": "not-a-cron",
            "flow_id": "test"
        });
        let err = trigger.validate_config(&config).unwrap_err();
        assert!(err[0].contains("invalid cron expression"));
    }

    #[test]
    fn test_cron_valid_schedule() {
        let trigger = CronTrigger;
        let config = json!({
            "schedule": "*/5 * * * *",
            "flow_id": "test"
        });
        assert!(trigger.validate_config(&config).is_ok());
    }

    #[tokio::test]
    async fn test_cron_emits_event() {
        tokio::time::pause();

        let (tx, mut rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Every second.
        let config = json!({
            "schedule": "* * * * * * *",
            "flow_id": "cron-flow"
        });

        let trigger = CronTrigger;
        let handle = tokio::spawn(async move { trigger.start(config, tx, shutdown_rx).await });

        // Advance time past the next cron tick.
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        tokio::task::yield_now().await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");

        assert_eq!(event.flow_id, "cron-flow");
        assert!(event.idempotency_key.is_some());

        // Verify idempotency key format.
        let key = event.idempotency_key.unwrap();
        assert!(key.starts_with("cron:cron-flow:"), "key was: {key}");

        shutdown_tx.send(()).expect("send shutdown");
        handle.await.expect("task completes").expect("no error");
    }

    #[tokio::test]
    async fn test_cron_idempotency_key_format() {
        tokio::time::pause();

        let (tx, mut rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let config = json!({
            "schedule": "* * * * * * *",
            "flow_id": "my-flow"
        });

        let trigger = CronTrigger;
        let handle = tokio::spawn(async move { trigger.start(config, tx, shutdown_rx).await });

        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        tokio::task::yield_now().await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");

        let key = event.idempotency_key.expect("should have key");
        // Format: cron:{flow_id}:{epoch_secs}
        let parts: Vec<&str> = key.splitn(3, ':').collect();
        assert_eq!(parts[0], "cron");
        assert_eq!(parts[1], "my-flow");
        // Third part should be a valid epoch timestamp.
        parts[2].parse::<i64>().expect("epoch should be a number");

        shutdown_tx.send(()).expect("send shutdown");
        handle.await.expect("task completes").expect("no error");
    }

    #[tokio::test]
    async fn test_cron_shutdown() {
        tokio::time::pause();

        let (tx, _rx) = mpsc::channel(10);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Far-future schedule (hourly) so it won't fire.
        let config = json!({
            "schedule": "0 0 * * * * *",
            "flow_id": "test"
        });

        let trigger = CronTrigger;
        let handle = tokio::spawn(async move { trigger.start(config, tx, shutdown_rx).await });

        // Allow the trigger to start waiting.
        tokio::task::yield_now().await;

        // Signal shutdown.
        shutdown_tx.send(()).expect("send shutdown");

        let result = handle.await.expect("task completes");
        assert!(result.is_ok());
    }
}
