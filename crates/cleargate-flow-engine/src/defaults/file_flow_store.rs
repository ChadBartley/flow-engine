//! File-system backed flow store.
//!
//! Layout:
//! ```text
//! {base_dir}/flows/{flow_id}.json     — FlowHead
//! {base_dir}/versions/{version_id}.json — FlowVersion
//! ```

use std::path::PathBuf;

use async_trait::async_trait;

use crate::errors::FlowStoreError;
use crate::traits::FlowStore;
use crate::types::{FlowHead, FlowVersion};

/// File-system backed store for flow definitions and versions.
///
/// Atomic writes use a temp-file-then-rename pattern to prevent
/// partial writes from corrupting the store.
pub struct FileFlowStore {
    flows_dir: PathBuf,
    versions_dir: PathBuf,
}

impl FileFlowStore {
    /// Create a new `FileFlowStore` rooted at `base_dir`.
    ///
    /// Creates `{base_dir}/flows/` and `{base_dir}/versions/` directories
    /// if they don't exist.
    pub fn new(base_dir: PathBuf) -> Result<Self, FlowStoreError> {
        let flows_dir = base_dir.join("flows");
        let versions_dir = base_dir.join("versions");

        std::fs::create_dir_all(&flows_dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to create flows directory: {e}"),
        })?;
        std::fs::create_dir_all(&versions_dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to create versions directory: {e}"),
        })?;

        Ok(Self {
            flows_dir,
            versions_dir,
        })
    }
}

/// Atomic write: serialize to temp file, then rename over the target.
fn atomic_write(path: &std::path::Path, data: &[u8]) -> Result<(), FlowStoreError> {
    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, data).map_err(|e| FlowStoreError::Store {
        message: format!("failed to write temp file: {e}"),
    })?;
    std::fs::rename(&temp_path, path).map_err(|e| FlowStoreError::Store {
        message: format!("failed to rename temp file: {e}"),
    })?;
    Ok(())
}

#[async_trait]
impl FlowStore for FileFlowStore {
    async fn put_version(&self, version: &FlowVersion) -> Result<(), FlowStoreError> {
        let path = self
            .versions_dir
            .join(format!("{}.json", version.version_id));
        let data = serde_json::to_vec_pretty(version).map_err(|e| FlowStoreError::Store {
            message: format!("failed to serialize version: {e}"),
        })?;
        atomic_write(&path, &data)
    }

    async fn put_head(&self, head: &FlowHead) -> Result<(), FlowStoreError> {
        let path = self.flows_dir.join(format!("{}.json", head.flow_id));
        let data = serde_json::to_vec_pretty(head).map_err(|e| FlowStoreError::Store {
            message: format!("failed to serialize head: {e}"),
        })?;
        atomic_write(&path, &data)
    }

    async fn get_head(&self, flow_id: &str) -> Result<Option<FlowHead>, FlowStoreError> {
        let path = self.flows_dir.join(format!("{flow_id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read(&path).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read flow head: {e}"),
        })?;
        let head = serde_json::from_slice(&data).map_err(|e| FlowStoreError::Store {
            message: format!("failed to deserialize flow head: {e}"),
        })?;
        Ok(Some(head))
    }

    async fn get_version(&self, version_id: &str) -> Result<Option<FlowVersion>, FlowStoreError> {
        let path = self.versions_dir.join(format!("{version_id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read(&path).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read flow version: {e}"),
        })?;
        let version = serde_json::from_slice(&data).map_err(|e| FlowStoreError::Store {
            message: format!("failed to deserialize flow version: {e}"),
        })?;
        Ok(Some(version))
    }

    async fn list_versions(
        &self,
        flow_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FlowVersion>, FlowStoreError> {
        let mut versions = Vec::new();

        let entries = std::fs::read_dir(&self.versions_dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read versions directory: {e}"),
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| FlowStoreError::Store {
                message: format!("failed to read dir entry: {e}"),
            })?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let data = std::fs::read(&path).map_err(|e| FlowStoreError::Store {
                message: format!("failed to read version file: {e}"),
            })?;
            if let Ok(version) = serde_json::from_slice::<FlowVersion>(&data) {
                if version.flow_id == flow_id {
                    versions.push(version);
                }
            }
        }

        // Sort by created_at descending, version_id desc as tiebreaker.
        versions.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.version_id.cmp(&a.version_id))
        });

        Ok(versions.into_iter().skip(offset).take(limit).collect())
    }

    async fn list_flows(&self) -> Result<Vec<FlowHead>, FlowStoreError> {
        let mut flows = Vec::new();

        let entries = std::fs::read_dir(&self.flows_dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read flows directory: {e}"),
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| FlowStoreError::Store {
                message: format!("failed to read dir entry: {e}"),
            })?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let data = std::fs::read(&path).map_err(|e| FlowStoreError::Store {
                message: format!("failed to read flow file: {e}"),
            })?;
            if let Ok(head) = serde_json::from_slice::<FlowHead>(&data) {
                flows.push(head);
            }
        }

        Ok(flows)
    }

    async fn get_versions_for_diff(
        &self,
        a: &str,
        b: &str,
    ) -> Result<(FlowVersion, FlowVersion), FlowStoreError> {
        let va = self
            .get_version(a)
            .await?
            .ok_or_else(|| FlowStoreError::NotFound { id: a.to_string() })?;
        let vb = self
            .get_version(b)
            .await?
            .ok_or_else(|| FlowStoreError::NotFound { id: b.to_string() })?;
        Ok((va, vb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GraphDef, GRAPH_SCHEMA_VERSION};
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn make_version(flow_id: &str, version_id: &str, secs_offset: i64) -> FlowVersion {
        FlowVersion {
            version_id: version_id.to_string(),
            flow_id: flow_id.to_string(),
            graph: GraphDef {
                schema_version: GRAPH_SCHEMA_VERSION,
                id: flow_id.to_string(),
                name: "Test".into(),
                version: version_id.to_string(),
                nodes: vec![],
                edges: vec![],
                metadata: BTreeMap::new(),
                tool_definitions: BTreeMap::new(),
            },
            created_at: Utc::now() + chrono::Duration::try_seconds(secs_offset).unwrap_or_default(),
            created_by: None,
            message: None,
        }
    }

    fn make_head(flow_id: &str, version_id: &str) -> FlowHead {
        FlowHead {
            flow_id: flow_id.to_string(),
            name: format!("Flow {flow_id}"),
            current_version_id: version_id.to_string(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_put_get_head() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileFlowStore::new(dir.path().to_path_buf()).unwrap();

        let head = make_head("flow-1", "v1");
        store.put_head(&head).await.unwrap();

        let loaded = store.get_head("flow-1").await.unwrap().unwrap();
        assert_eq!(loaded.flow_id, "flow-1");
        assert_eq!(loaded.current_version_id, "v1");
    }

    #[tokio::test]
    async fn test_put_get_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileFlowStore::new(dir.path().to_path_buf()).unwrap();

        let version = make_version("flow-1", "v1", 0);
        store.put_version(&version).await.unwrap();

        let loaded = store.get_version("v1").await.unwrap().unwrap();
        assert_eq!(loaded.version_id, "v1");
        assert_eq!(loaded.flow_id, "flow-1");
    }

    #[tokio::test]
    async fn test_list_versions_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileFlowStore::new(dir.path().to_path_buf()).unwrap();

        // Insert in non-chronological order.
        let v1 = make_version("flow-1", "v1", 0);
        let v2 = make_version("flow-1", "v2", 10);
        let v3 = make_version("flow-1", "v3", 5);
        store.put_version(&v1).await.unwrap();
        store.put_version(&v2).await.unwrap();
        store.put_version(&v3).await.unwrap();

        let listed = store.list_versions("flow-1", 10, 0).await.unwrap();
        assert_eq!(listed.len(), 3);
        // Most recent first.
        assert_eq!(listed[0].version_id, "v2");
        assert_eq!(listed[1].version_id, "v3");
        assert_eq!(listed[2].version_id, "v1");
    }

    #[tokio::test]
    async fn test_list_flows() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileFlowStore::new(dir.path().to_path_buf()).unwrap();

        store.put_head(&make_head("flow-a", "v1")).await.unwrap();
        store.put_head(&make_head("flow-b", "v2")).await.unwrap();

        let flows = store.list_flows().await.unwrap();
        assert_eq!(flows.len(), 2);
        let ids: Vec<&str> = flows.iter().map(|f| f.flow_id.as_str()).collect();
        assert!(ids.contains(&"flow-a"));
        assert!(ids.contains(&"flow-b"));
    }

    #[tokio::test]
    async fn test_get_versions_for_diff() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileFlowStore::new(dir.path().to_path_buf()).unwrap();

        store
            .put_version(&make_version("flow-1", "va", 0))
            .await
            .unwrap();
        store
            .put_version(&make_version("flow-1", "vb", 1))
            .await
            .unwrap();

        let (a, b) = store.get_versions_for_diff("va", "vb").await.unwrap();
        assert_eq!(a.version_id, "va");
        assert_eq!(b.version_id, "vb");
    }

    #[tokio::test]
    async fn test_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileFlowStore::new(dir.path().to_path_buf()).unwrap();

        assert!(store.get_head("nonexistent").await.unwrap().is_none());
        assert!(store.get_version("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_concurrent_writes_same_flow() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(FileFlowStore::new(dir.path().to_path_buf()).unwrap());

        // 10 tasks writing different versions concurrently.
        let mut handles = Vec::new();
        for i in 0..10u32 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                let vid = format!("v{i}");
                let version = FlowVersion {
                    version_id: vid.clone(),
                    flow_id: "flow-1".to_string(),
                    graph: GraphDef {
                        schema_version: GRAPH_SCHEMA_VERSION,
                        id: "flow-1".to_string(),
                        name: "Test".into(),
                        version: vid.clone(),
                        nodes: vec![],
                        edges: vec![],
                        metadata: BTreeMap::new(),
                        tool_definitions: BTreeMap::new(),
                    },
                    created_at: Utc::now(),
                    created_by: None,
                    message: None,
                };
                store.put_version(&version).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // All 10 versions should be retrievable.
        let versions = store.list_versions("flow-1", 100, 0).await.unwrap();
        assert_eq!(versions.len(), 10, "all 10 versions should be stored");

        // Each version should be individually retrievable.
        for i in 0..10u32 {
            let vid = format!("v{i}");
            let v = store.get_version(&vid).await.unwrap();
            assert!(v.is_some(), "version {vid} should exist");
        }
    }
}
