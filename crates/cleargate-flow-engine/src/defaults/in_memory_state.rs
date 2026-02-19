//! In-memory state store for execution scratchpad and checkpointing.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::errors::StateError;
use crate::traits::StateStore;
use crate::types::Sensitivity;

/// Per-run state: maps key → (value, sensitivity).
type RunState = HashMap<String, (Value, Sensitivity)>;

/// In-memory state store backed by a `HashMap` protected by `RwLock`.
///
/// State is keyed by `(run_id, key)`. Sensitivity metadata is stored
/// alongside each value but not used for access control — the
/// [`Redactor`](crate::traits::Redactor) handles scrubbing at
/// the write pipeline layer.
pub struct InMemoryState {
    state: RwLock<HashMap<String, RunState>>,
}

impl InMemoryState {
    /// Create a new empty in-memory state store.
    pub fn new() -> Self {
        Self {
            state: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryState {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StateStore for InMemoryState {
    async fn get(&self, run_id: &str, key: &str) -> Result<Option<Value>, StateError> {
        let guard = self.state.read().await;
        Ok(guard
            .get(run_id)
            .and_then(|run| run.get(key))
            .map(|(v, _)| v.clone()))
    }

    async fn set(
        &self,
        run_id: &str,
        key: &str,
        value: Value,
        sensitivity: Sensitivity,
    ) -> Result<(), StateError> {
        let mut guard = self.state.write().await;
        guard
            .entry(run_id.to_string())
            .or_default()
            .insert(key.to_string(), (value, sensitivity));
        Ok(())
    }

    async fn snapshot(&self, run_id: &str) -> Result<HashMap<String, Value>, StateError> {
        let guard = self.state.read().await;
        Ok(guard
            .get(run_id)
            .map(|run| {
                run.iter()
                    .map(|(k, (v, _))| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn restore(
        &self,
        run_id: &str,
        snapshot: HashMap<String, Value>,
    ) -> Result<(), StateError> {
        let mut guard = self.state.write().await;
        let run = guard.entry(run_id.to_string()).or_default();
        run.clear();
        for (k, v) in snapshot {
            run.insert(k, (v, Sensitivity::None));
        }
        Ok(())
    }

    async fn cleanup(&self, run_id: &str) -> Result<(), StateError> {
        let mut guard = self.state.write().await;
        guard.remove(run_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_get_set() {
        let store = InMemoryState::new();
        store
            .set("run-1", "counter", json!(42), Sensitivity::None)
            .await
            .unwrap();
        let val = store.get("run-1", "counter").await.unwrap();
        assert_eq!(val, Some(json!(42)));
    }

    #[tokio::test]
    async fn test_get_missing() {
        let store = InMemoryState::new();
        let val = store.get("run-1", "missing").await.unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn test_snapshot() {
        let store = InMemoryState::new();
        store
            .set("run-1", "a", json!(1), Sensitivity::None)
            .await
            .unwrap();
        store
            .set("run-1", "b", json!(2), Sensitivity::Secret)
            .await
            .unwrap();

        let snap = store.snapshot("run-1").await.unwrap();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap["a"], json!(1));
        assert_eq!(snap["b"], json!(2));
    }

    #[tokio::test]
    async fn test_restore() {
        let store = InMemoryState::new();
        store
            .set("run-1", "old_key", json!("old"), Sensitivity::None)
            .await
            .unwrap();

        let mut snap = HashMap::new();
        snap.insert("new_key".to_string(), json!("new"));
        store.restore("run-1", snap).await.unwrap();

        assert_eq!(store.get("run-1", "old_key").await.unwrap(), None);
        assert_eq!(
            store.get("run-1", "new_key").await.unwrap(),
            Some(json!("new"))
        );
    }

    #[tokio::test]
    async fn test_cleanup() {
        let store = InMemoryState::new();
        store
            .set("run-1", "key", json!("val"), Sensitivity::None)
            .await
            .unwrap();
        store.cleanup("run-1").await.unwrap();
        assert_eq!(store.get("run-1", "key").await.unwrap(), None);
    }
}
