//! Namespaced shared memory for cross-agent communication.
//!
//! [`Blackboard`] wraps the existing [`StateStore`] with namespace-prefixed
//! keys, giving each agent its own keyspace within a shared run. Supervisors
//! can read across namespaces to aggregate worker results; workers write to
//! their own namespace to avoid key collisions.
//!
//! No new traits or storage backends are needed — the blackboard is a thin
//! semantic layer over the engine's existing state infrastructure.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::traits::StateStore;
use crate::types::{NodeError, Sensitivity};

/// Namespaced shared-memory view over a run's [`StateStore`].
///
/// Keys are internally prefixed as `"{namespace}:{key}"` so that
/// multiple agents can share state without collisions. The blackboard
/// does not own the state — it borrows the run's existing store.
pub struct Blackboard {
    state: Arc<dyn StateStore>,
    run_id: String,
}

impl Blackboard {
    /// Create a blackboard scoped to a specific run.
    pub fn new(state: Arc<dyn StateStore>, run_id: String) -> Self {
        Self { state, run_id }
    }

    /// Read a value from the given namespace.
    pub async fn read(&self, namespace: &str, key: &str) -> Result<Option<Value>, NodeError> {
        let full_key = format!("{namespace}:{key}");
        self.state
            .get(&self.run_id, &full_key)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("blackboard read error: {e}"),
            })
    }

    /// Write a value into the given namespace.
    pub async fn write(&self, namespace: &str, key: &str, value: Value) -> Result<(), NodeError> {
        let full_key = format!("{namespace}:{key}");
        self.state
            .set(&self.run_id, &full_key, value, Sensitivity::None)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("blackboard write error: {e}"),
            })
    }

    /// Read all key-value pairs within a namespace.
    ///
    /// Returns keys with the namespace prefix stripped.
    pub async fn read_all(&self, namespace: &str) -> Result<HashMap<String, Value>, NodeError> {
        let prefix = format!("{namespace}:");
        let snapshot = self
            .state
            .snapshot(&self.run_id)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("blackboard snapshot error: {e}"),
            })?;

        Ok(snapshot
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|rest| (rest.to_string(), v)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::InMemoryState;
    use serde_json::json;

    fn make_blackboard() -> Blackboard {
        Blackboard::new(Arc::new(InMemoryState::new()), "run-1".into())
    }

    #[tokio::test]
    async fn write_and_read() {
        let bb = make_blackboard();
        bb.write("researcher", "result", json!("found it"))
            .await
            .unwrap();

        let val = bb.read("researcher", "result").await.unwrap();
        assert_eq!(val, Some(json!("found it")));
    }

    #[tokio::test]
    async fn read_missing_returns_none() {
        let bb = make_blackboard();
        let val = bb.read("researcher", "nonexistent").await.unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn namespace_isolation() {
        let bb = make_blackboard();
        bb.write("agent_a", "key", json!("value_a")).await.unwrap();
        bb.write("agent_b", "key", json!("value_b")).await.unwrap();

        assert_eq!(
            bb.read("agent_a", "key").await.unwrap(),
            Some(json!("value_a"))
        );
        assert_eq!(
            bb.read("agent_b", "key").await.unwrap(),
            Some(json!("value_b"))
        );
    }

    #[tokio::test]
    async fn read_all_filters_by_namespace() {
        let bb = make_blackboard();
        bb.write("researcher", "result_1", json!("a"))
            .await
            .unwrap();
        bb.write("researcher", "result_2", json!("b"))
            .await
            .unwrap();
        bb.write("writer", "draft", json!("c")).await.unwrap();

        let researcher_data = bb.read_all("researcher").await.unwrap();
        assert_eq!(researcher_data.len(), 2);
        assert_eq!(researcher_data.get("result_1"), Some(&json!("a")));
        assert_eq!(researcher_data.get("result_2"), Some(&json!("b")));
    }

    #[tokio::test]
    async fn read_all_empty_namespace() {
        let bb = make_blackboard();
        let data = bb.read_all("empty").await.unwrap();
        assert!(data.is_empty());
    }

    #[tokio::test]
    async fn overwrite_existing_key() {
        let bb = make_blackboard();
        bb.write("ns", "key", json!(1)).await.unwrap();
        bb.write("ns", "key", json!(2)).await.unwrap();

        assert_eq!(bb.read("ns", "key").await.unwrap(), Some(json!(2)));
    }
}
