//! Content-addressed flow versioning.
//!
//! Computes deterministic SHA-256 hashes for flow graphs and manages
//! the save/retrieve lifecycle through the [`FlowStore`] trait.
//!
//! **Invariant I3**: Same graph = same hash. `BTreeMap` in [`GraphDef`]
//! guarantees deterministic serialization; `serde_json::to_vec` produces
//! canonical bytes.

use std::sync::Arc;

use chrono::Utc;
use sha2::{Digest, Sha256};

use super::errors::FlowStoreError;
use super::traits::FlowStore;
use super::types::{FlowHead, FlowVersion, GraphDef};

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Compute a content-addressed version ID for a graph.
///
/// Serializes the [`GraphDef`] to canonical JSON bytes (BTreeMap guarantees
/// deterministic key order), then returns the lowercase hex SHA-256 hash.
/// `tool_definitions` are part of the graph, so they're included in the hash.
pub fn compute_version_id(graph: &GraphDef) -> String {
    let bytes = serde_json::to_vec(graph).expect("GraphDef serialization should never fail");
    let hash = Sha256::digest(&bytes);
    format!("{hash:x}")
}

/// Compute a config hash for a graph's resolved node configurations.
///
/// For each `NodeInstance` in `graph.nodes`, the config `Value` is included.
/// Any key path containing "SECRET", "KEY", or "PASSWORD" (case-insensitive)
/// has its value replaced with `"[SECRET_PLACEHOLDER]"` before hashing.
///
/// This ensures:
/// - `config_hash` changes when non-secret config changes (e.g. model name).
/// - `config_hash` stays stable when only secret values rotate.
pub fn compute_config_hash(graph: &GraphDef) -> String {
    let mut configs: Vec<serde_json::Value> = graph
        .nodes
        .iter()
        .map(|n| sanitize_secrets(&n.config))
        .collect();
    // Sort by serialized form for determinism (node order may vary).
    configs.sort_by(|a, b| {
        let sa = serde_json::to_string(a).unwrap_or_default();
        let sb = serde_json::to_string(b).unwrap_or_default();
        sa.cmp(&sb)
    });
    let bytes = serde_json::to_vec(&configs).expect("config serialization should never fail");
    let hash = Sha256::digest(&bytes);
    format!("{hash:x}")
}

/// Replace values whose key contains "SECRET", "KEY", or "PASSWORD"
/// (case-insensitive) with `"[SECRET_PLACEHOLDER]"`.
fn sanitize_secrets(value: &serde_json::Value) -> serde_json::Value {
    sanitize_value(value, "")
}

fn sanitize_value(value: &serde_json::Value, key_path: &str) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let path = if key_path.is_empty() {
                    k.clone()
                } else {
                    format!("{key_path}.{k}")
                };
                if is_secret_key(k) {
                    out.insert(
                        k.clone(),
                        serde_json::Value::String("[SECRET_PLACEHOLDER]".into()),
                    );
                } else {
                    out.insert(k.clone(), sanitize_value(v, &path));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(|v| sanitize_value(v, key_path)).collect())
        }
        other => other.clone(),
    }
}

fn is_secret_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    upper.contains("SECRET") || upper.contains("KEY") || upper.contains("PASSWORD")
}

// ---------------------------------------------------------------------------
// FlowVersioning service
// ---------------------------------------------------------------------------

/// High-level versioning service over a [`FlowStore`].
///
/// Handles deduplication (same hash = no new version) and FlowHead updates.
pub struct FlowVersioning {
    store: Arc<dyn FlowStore>,
}

impl FlowVersioning {
    /// Create a new versioning service backed by the given store.
    pub fn new(store: Arc<dyn FlowStore>) -> Self {
        Self { store }
    }

    /// Save a new flow version (or return the existing one if the graph is
    /// unchanged — same hash = no duplicate).
    ///
    /// Updates the [`FlowHead`] to point at the new version.
    pub async fn save(
        &self,
        flow_id: &str,
        name: &str,
        graph: GraphDef,
        created_by: Option<String>,
        message: Option<String>,
    ) -> Result<FlowVersion, FlowStoreError> {
        let version_id = compute_version_id(&graph);

        // Dedup: if this exact version already exists, return it.
        if let Some(existing) = self.store.get_version(&version_id).await? {
            return Ok(existing);
        }

        let version = FlowVersion {
            version_id: version_id.clone(),
            flow_id: flow_id.to_string(),
            graph,
            created_at: Utc::now(),
            created_by,
            message,
        };

        self.store.put_version(&version).await?;

        let head = FlowHead {
            flow_id: flow_id.to_string(),
            name: name.to_string(),
            current_version_id: version_id,
            updated_at: Utc::now(),
        };
        self.store.put_head(&head).await?;

        Ok(version)
    }

    /// Get the current (HEAD) version of a flow.
    pub async fn get_current(&self, flow_id: &str) -> Result<Option<FlowVersion>, FlowStoreError> {
        let head = match self.store.get_head(flow_id).await? {
            Some(h) => h,
            None => return Ok(None),
        };
        self.store.get_version(&head.current_version_id).await
    }

    /// Retrieve a specific historical version by its content-addressed ID.
    pub async fn get_at_version(
        &self,
        version_id: &str,
    ) -> Result<Option<FlowVersion>, FlowStoreError> {
        self.store.get_version(version_id).await
    }

    /// List version history for a flow, sorted by `created_at` descending.
    pub async fn history(
        &self,
        flow_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FlowVersion>, FlowStoreError> {
        self.store.list_versions(flow_id, limit, offset).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::FileFlowStore;
    use crate::types::{NodeInstance, ToolDef, ToolType, GRAPH_SCHEMA_VERSION};
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    /// Helper: build a minimal GraphDef.
    fn make_graph(id: &str, tool_defs: BTreeMap<String, ToolDef>) -> GraphDef {
        GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: id.into(),
            name: "Test Flow".into(),
            version: "1".into(),
            nodes: vec![],
            edges: vec![],
            metadata: BTreeMap::new(),
            tool_definitions: tool_defs,
        }
    }

    fn make_tool(name: &str, description: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: description.into(),
            parameters: json!({"type": "object"}),
            tool_type: ToolType::Node {
                target_node_id: "n1".into(),
            },
            metadata: BTreeMap::new(),
            permissions: BTreeSet::new(),
        }
    }

    fn make_graph_with_nodes(nodes: Vec<NodeInstance>) -> GraphDef {
        GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "flow-1".into(),
            name: "Test".into(),
            version: "1".into(),
            nodes,
            edges: vec![],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        }
    }

    // -- Versioning function tests --

    #[test]
    fn test_deterministic_hashing() {
        let graph = make_graph("flow-1", BTreeMap::new());
        let hash1 = compute_version_id(&graph);
        let hash2 = compute_version_id(&graph);
        assert_eq!(hash1, hash2, "same graph must produce same hash");
        assert_eq!(hash1.len(), 64, "SHA-256 hex should be 64 chars");
    }

    #[test]
    fn test_same_tools_same_hash() {
        let mut tools = BTreeMap::new();
        tools.insert("search".into(), make_tool("search", "Search the web"));
        let graph_a = make_graph("flow-1", tools.clone());
        let graph_b = make_graph("flow-1", tools);
        assert_eq!(compute_version_id(&graph_a), compute_version_id(&graph_b));
    }

    #[test]
    fn test_different_tools_different_hash() {
        let mut tools_a = BTreeMap::new();
        tools_a.insert("search".into(), make_tool("search", "Search the web"));

        let mut tools_b = BTreeMap::new();
        tools_b.insert("search".into(), make_tool("search", "Search the web v2"));

        let graph_a = make_graph("flow-1", tools_a);
        let graph_b = make_graph("flow-1", tools_b);
        assert_ne!(
            compute_version_id(&graph_a),
            compute_version_id(&graph_b),
            "different tool descriptions must produce different hashes"
        );
    }

    #[test]
    fn test_tool_order_irrelevant() {
        // BTreeMap sorts by key, so insertion order shouldn't matter.
        let mut tools_a = BTreeMap::new();
        tools_a.insert("z_tool".into(), make_tool("z_tool", "Z"));
        tools_a.insert("a_tool".into(), make_tool("a_tool", "A"));

        let mut tools_b = BTreeMap::new();
        tools_b.insert("a_tool".into(), make_tool("a_tool", "A"));
        tools_b.insert("z_tool".into(), make_tool("z_tool", "Z"));

        let graph_a = make_graph("flow-1", tools_a);
        let graph_b = make_graph("flow-1", tools_b);
        assert_eq!(
            compute_version_id(&graph_a),
            compute_version_id(&graph_b),
            "BTreeMap insertion order must not affect the hash"
        );
    }

    #[test]
    fn test_config_hash_changes_with_config() {
        let graph_a = make_graph_with_nodes(vec![NodeInstance {
            instance_id: "n1".into(),
            node_type: "llm_call".into(),
            config: json!({"model": "gpt-4o", "temperature": 0.7}),
            position: None,
            tool_access: None,
        }]);
        let graph_b = make_graph_with_nodes(vec![NodeInstance {
            instance_id: "n1".into(),
            node_type: "llm_call".into(),
            config: json!({"model": "claude-sonnet", "temperature": 0.7}),
            position: None,
            tool_access: None,
        }]);

        assert_ne!(
            compute_config_hash(&graph_a),
            compute_config_hash(&graph_b),
            "different model names must produce different config hashes"
        );
    }

    #[test]
    fn test_config_hash_stable_across_secret_values() {
        let graph_a = make_graph_with_nodes(vec![NodeInstance {
            instance_id: "n1".into(),
            node_type: "llm_call".into(),
            config: json!({"model": "gpt-4o", "api_key": "sk-old-value"}),
            position: None,
            tool_access: None,
        }]);
        let graph_b = make_graph_with_nodes(vec![NodeInstance {
            instance_id: "n1".into(),
            node_type: "llm_call".into(),
            config: json!({"model": "gpt-4o", "api_key": "sk-new-value"}),
            position: None,
            tool_access: None,
        }]);

        assert_eq!(
            compute_config_hash(&graph_a),
            compute_config_hash(&graph_b),
            "changing a value under a 'key' key must not change config_hash"
        );
    }

    // -- FlowVersioning service tests --

    #[tokio::test]
    async fn test_no_duplicate_versions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileFlowStore::new(dir.path().to_path_buf()).unwrap());
        let svc = FlowVersioning::new(store.clone());

        let graph = make_graph("flow-1", BTreeMap::new());
        let v1 = svc
            .save("flow-1", "Test", graph.clone(), None, None)
            .await
            .unwrap();
        let v2 = svc.save("flow-1", "Test", graph, None, None).await.unwrap();

        assert_eq!(v1.version_id, v2.version_id, "same graph = same version");

        // Only one version should be stored.
        let versions = store.list_versions("flow-1", 100, 0).await.unwrap();
        assert_eq!(versions.len(), 1);
    }

    #[tokio::test]
    async fn test_version_history_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileFlowStore::new(dir.path().to_path_buf()).unwrap());
        let svc = FlowVersioning::new(store);

        // Save 3 distinct versions.
        for i in 0..3 {
            let mut graph = make_graph("flow-1", BTreeMap::new());
            graph.version = format!("v{i}");
            svc.save("flow-1", "Test", graph, None, Some(format!("v{i}")))
                .await
                .unwrap();
            // Small delay to ensure different created_at timestamps.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let history = svc.history("flow-1", 100, 0).await.unwrap();
        assert_eq!(history.len(), 3);
        // Most recent first.
        for i in 0..history.len() - 1 {
            assert!(
                history[i].created_at >= history[i + 1].created_at,
                "history should be sorted by created_at descending"
            );
        }
    }

    #[tokio::test]
    async fn test_save_updates_head() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileFlowStore::new(dir.path().to_path_buf()).unwrap());
        let svc = FlowVersioning::new(store);

        let graph_a = make_graph("flow-1", BTreeMap::new());
        svc.save("flow-1", "Test", graph_a, None, None)
            .await
            .unwrap();

        let mut graph_b = make_graph("flow-1", BTreeMap::new());
        graph_b.version = "2".into();
        let v2 = svc
            .save("flow-1", "Test v2", graph_b, None, None)
            .await
            .unwrap();

        let current = svc.get_current("flow-1").await.unwrap().unwrap();
        assert_eq!(current.version_id, v2.version_id);
    }

    #[tokio::test]
    async fn test_get_at_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileFlowStore::new(dir.path().to_path_buf()).unwrap());
        let svc = FlowVersioning::new(store);

        let graph = make_graph("flow-1", BTreeMap::new());
        let saved = svc.save("flow-1", "Test", graph, None, None).await.unwrap();

        let retrieved = svc
            .get_at_version(&saved.version_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.version_id, saved.version_id);
        assert_eq!(retrieved.flow_id, "flow-1");
    }
}
