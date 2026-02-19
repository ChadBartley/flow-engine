//! SeaORM-backed flow store.
//!
//! Stores flow heads and versions in the database using the `flow_head`
//! and `flow_version` tables.

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, DatabaseConnection, QueryOrder, QuerySelect};
use std::sync::Arc;

use crate::models::{flow_head, flow_version};

use cleargate_flow_engine::errors::FlowStoreError;
use cleargate_flow_engine::traits::FlowStore;
use cleargate_flow_engine::types::{FlowHead, FlowVersion, GraphDef};

/// Database-backed store for flow definitions and versions.
pub struct SeaOrmFlowStore {
    db: Arc<DatabaseConnection>,
}

impl SeaOrmFlowStore {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

fn to_store_err(e: impl std::fmt::Display) -> FlowStoreError {
    FlowStoreError::Store {
        message: e.to_string(),
    }
}

fn model_to_head(m: flow_head::Model) -> FlowHead {
    FlowHead {
        flow_id: m.flow_id,
        name: m.name,
        current_version_id: m.current_version_id,
        updated_at: m.updated_at.and_utc(),
    }
}

fn model_to_version(m: flow_version::Model) -> Result<FlowVersion, FlowStoreError> {
    let graph: GraphDef = serde_json::from_value(m.graph).map_err(to_store_err)?;
    Ok(FlowVersion {
        version_id: m.version_id,
        flow_id: m.flow_id,
        graph,
        created_at: m.created_at.and_utc(),
        created_by: m.created_by,
        message: m.message,
    })
}

#[async_trait]
impl FlowStore for SeaOrmFlowStore {
    async fn put_version(&self, version: &FlowVersion) -> Result<(), FlowStoreError> {
        let graph_json = serde_json::to_value(&version.graph).map_err(to_store_err)?;
        let model = flow_version::ActiveModel {
            version_id: ActiveValue::Set(version.version_id.clone()),
            flow_id: ActiveValue::Set(version.flow_id.clone()),
            graph: ActiveValue::Set(graph_json),
            created_at: ActiveValue::Set(version.created_at.naive_utc()),
            created_by: ActiveValue::Set(version.created_by.clone()),
            message: ActiveValue::Set(version.message.clone()),
        };
        flow_version::Entity::insert(model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(flow_version::Column::VersionId)
                    .do_nothing()
                    .to_owned(),
            )
            .do_nothing()
            .exec(self.db.as_ref())
            .await
            .map_err(to_store_err)?;
        Ok(())
    }

    async fn put_head(&self, head: &FlowHead) -> Result<(), FlowStoreError> {
        let model = flow_head::ActiveModel {
            flow_id: ActiveValue::Set(head.flow_id.clone()),
            name: ActiveValue::Set(head.name.clone()),
            current_version_id: ActiveValue::Set(head.current_version_id.clone()),
            updated_at: ActiveValue::Set(head.updated_at.naive_utc()),
        };
        flow_head::Entity::insert(model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(flow_head::Column::FlowId)
                    .update_columns([
                        flow_head::Column::Name,
                        flow_head::Column::CurrentVersionId,
                        flow_head::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(self.db.as_ref())
            .await
            .map_err(to_store_err)?;
        Ok(())
    }

    async fn get_head(&self, flow_id: &str) -> Result<Option<FlowHead>, FlowStoreError> {
        let result = flow_head::Entity::find_by_id(flow_id)
            .one(self.db.as_ref())
            .await
            .map_err(to_store_err)?;
        Ok(result.map(model_to_head))
    }

    async fn get_version(&self, version_id: &str) -> Result<Option<FlowVersion>, FlowStoreError> {
        let result = flow_version::Entity::find_by_id(version_id)
            .one(self.db.as_ref())
            .await
            .map_err(to_store_err)?;
        result.map(model_to_version).transpose()
    }

    async fn list_versions(
        &self,
        flow_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FlowVersion>, FlowStoreError> {
        let models = flow_version::Entity::find()
            .filter(flow_version::Column::FlowId.eq(flow_id))
            .order_by_desc(flow_version::Column::CreatedAt)
            .order_by_desc(flow_version::Column::VersionId)
            .offset(Some(offset as u64))
            .limit(Some(limit as u64))
            .all(self.db.as_ref())
            .await
            .map_err(to_store_err)?;
        models.into_iter().map(model_to_version).collect()
    }

    async fn list_flows(&self) -> Result<Vec<FlowHead>, FlowStoreError> {
        let models = flow_head::Entity::find()
            .all(self.db.as_ref())
            .await
            .map_err(to_store_err)?;
        Ok(models.into_iter().map(model_to_head).collect())
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
    use chrono::Utc;
    use cleargate_flow_engine::types::{GraphDef, GRAPH_SCHEMA_VERSION};
    use sea_orm::Database;
    use sea_orm_migration::MigratorTrait;
    use std::collections::BTreeMap;

    async fn setup_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        crate::migrations::Migrator::up(&db, None).await.unwrap();
        db
    }

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
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

        let head = make_head("flow-1", "v1");
        store.put_head(&head).await.unwrap();

        let loaded = store.get_head("flow-1").await.unwrap().unwrap();
        assert_eq!(loaded.flow_id, "flow-1");
        assert_eq!(loaded.current_version_id, "v1");
    }

    #[tokio::test]
    async fn test_put_get_version() {
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

        let version = make_version("flow-1", "v1", 0);
        store.put_version(&version).await.unwrap();

        let loaded = store.get_version("v1").await.unwrap().unwrap();
        assert_eq!(loaded.version_id, "v1");
        assert_eq!(loaded.flow_id, "flow-1");
    }

    #[tokio::test]
    async fn test_list_versions_sorted() {
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

        let v1 = make_version("flow-1", "v1", 0);
        let v2 = make_version("flow-1", "v2", 10);
        let v3 = make_version("flow-1", "v3", 5);
        store.put_version(&v1).await.unwrap();
        store.put_version(&v2).await.unwrap();
        store.put_version(&v3).await.unwrap();

        let listed = store.list_versions("flow-1", 10, 0).await.unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].version_id, "v2");
        assert_eq!(listed[1].version_id, "v3");
        assert_eq!(listed[2].version_id, "v1");
    }

    #[tokio::test]
    async fn test_list_flows() {
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

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
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

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
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

        assert!(store.get_head("nonexistent").await.unwrap().is_none());
        assert!(store.get_version("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_put_head_upsert() {
        let db = setup_db().await;
        let store = SeaOrmFlowStore::new(Arc::new(db));

        store.put_head(&make_head("flow-1", "v1")).await.unwrap();
        store.put_head(&make_head("flow-1", "v2")).await.unwrap();

        let loaded = store.get_head("flow-1").await.unwrap().unwrap();
        assert_eq!(loaded.current_version_id, "v2");
    }
}
