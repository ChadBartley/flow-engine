//! File-system backed tag store.
//!
//! Layout:
//! ```text
//! {base_dir}/tags/{flow_id}/{tag}.json
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;

use crate::errors::FlowStoreError;
use crate::traits::TagStore;
use crate::types::FlowTag;

/// File-system backed store for flow version tags.
pub struct FileTagStore {
    tags_dir: PathBuf,
}

impl FileTagStore {
    pub fn new(base_dir: PathBuf) -> Result<Self, FlowStoreError> {
        let tags_dir = base_dir.join("tags");
        std::fs::create_dir_all(&tags_dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to create tags directory: {e}"),
        })?;
        Ok(Self { tags_dir })
    }

    fn flow_dir(&self, flow_id: &str) -> PathBuf {
        self.tags_dir.join(flow_id)
    }

    fn tag_path(&self, flow_id: &str, tag: &str) -> PathBuf {
        self.flow_dir(flow_id).join(format!("{tag}.json"))
    }
}

#[async_trait]
impl TagStore for FileTagStore {
    async fn set_tag(
        &self,
        flow_id: &str,
        tag: &str,
        version_id: &str,
    ) -> Result<(), FlowStoreError> {
        let dir = self.flow_dir(flow_id);
        std::fs::create_dir_all(&dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to create tag directory: {e}"),
        })?;

        let flow_tag = FlowTag {
            flow_id: flow_id.to_string(),
            tag: tag.to_string(),
            version_id: version_id.to_string(),
            updated_at: Utc::now(),
        };

        let data = serde_json::to_vec_pretty(&flow_tag).map_err(|e| FlowStoreError::Store {
            message: format!("failed to serialize tag: {e}"),
        })?;

        let path = self.tag_path(flow_id, tag);
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, &data).map_err(|e| FlowStoreError::Store {
            message: format!("failed to write tag file: {e}"),
        })?;
        std::fs::rename(&temp, &path).map_err(|e| FlowStoreError::Store {
            message: format!("failed to rename tag file: {e}"),
        })?;
        Ok(())
    }

    async fn get_tag(&self, flow_id: &str, tag: &str) -> Result<Option<String>, FlowStoreError> {
        let path = self.tag_path(flow_id, tag);
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read(&path).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read tag file: {e}"),
        })?;
        let flow_tag: FlowTag =
            serde_json::from_slice(&data).map_err(|e| FlowStoreError::Store {
                message: format!("failed to deserialize tag: {e}"),
            })?;
        Ok(Some(flow_tag.version_id))
    }

    async fn list_tags(&self, flow_id: &str) -> Result<Vec<FlowTag>, FlowStoreError> {
        let dir = self.flow_dir(flow_id);
        if !dir.exists() {
            return Ok(vec![]);
        }

        let entries = std::fs::read_dir(&dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read tags directory: {e}"),
        })?;

        let mut tags = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| FlowStoreError::Store {
                message: format!("failed to read dir entry: {e}"),
            })?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let data = std::fs::read(&path).map_err(|e| FlowStoreError::Store {
                message: format!("failed to read tag file: {e}"),
            })?;
            if let Ok(tag) = serde_json::from_slice::<FlowTag>(&data) {
                tags.push(tag);
            }
        }
        tags.sort_by(|a, b| a.tag.cmp(&b.tag));
        Ok(tags)
    }

    async fn delete_tag(&self, flow_id: &str, tag: &str) -> Result<bool, FlowStoreError> {
        let path = self.tag_path(flow_id, tag);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|e| FlowStoreError::Store {
            message: format!("failed to delete tag file: {e}"),
        })?;
        Ok(true)
    }

    async fn tags_for_version(&self, version_id: &str) -> Result<Vec<String>, FlowStoreError> {
        let mut result = Vec::new();
        if !self.tags_dir.exists() {
            return Ok(result);
        }

        let entries = std::fs::read_dir(&self.tags_dir).map_err(|e| FlowStoreError::Store {
            message: format!("failed to read tags directory: {e}"),
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| FlowStoreError::Store {
                message: format!("failed to read dir entry: {e}"),
            })?;
            if !entry.path().is_dir() {
                continue;
            }
            let flow_id = entry.file_name().to_string_lossy().to_string();
            let tags = self.list_tags(&flow_id).await?;
            for tag in tags {
                if tag.version_id == version_id {
                    result.push(tag.tag);
                }
            }
        }
        result.sort();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_set_get_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();

        store.set_tag("flow-1", "prod", "v123").await.unwrap();
        let vid = store.get_tag("flow-1", "prod").await.unwrap();
        assert_eq!(vid, Some("v123".to_string()));
    }

    #[tokio::test]
    async fn test_update_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();

        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        store.set_tag("flow-1", "prod", "v2").await.unwrap();
        let vid = store.get_tag("flow-1", "prod").await.unwrap();
        assert_eq!(vid, Some("v2".to_string()));
    }

    #[tokio::test]
    async fn test_list_tags() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();

        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        store.set_tag("flow-1", "staging", "v2").await.unwrap();

        let tags = store.list_tags("flow-1").await.unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tag, "prod");
        assert_eq!(tags[1].tag, "staging");
    }

    #[tokio::test]
    async fn test_delete_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();

        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        assert!(store.delete_tag("flow-1", "prod").await.unwrap());
        assert!(!store.delete_tag("flow-1", "prod").await.unwrap());
        assert!(store.get_tag("flow-1", "prod").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_tags_for_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();

        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        store.set_tag("flow-1", "latest", "v1").await.unwrap();
        store.set_tag("flow-2", "prod", "v1").await.unwrap();

        let tags = store.tags_for_version("v1").await.unwrap();
        assert_eq!(tags.len(), 3);
    }

    #[tokio::test]
    async fn test_missing_tag_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();
        assert!(store.get_tag("flow-1", "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTagStore::new(dir.path().to_path_buf()).unwrap();
        assert!(store.list_tags("flow-1").await.unwrap().is_empty());
    }
}
