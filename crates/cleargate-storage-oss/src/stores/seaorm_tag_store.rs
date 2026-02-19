//! SeaORM-backed tag store.

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, DatabaseConnection, QueryOrder};
use std::sync::Arc;

use crate::models::flow_tag;

use cleargate_flow_engine::errors::FlowStoreError;
use cleargate_flow_engine::traits::TagStore;
use cleargate_flow_engine::types::FlowTag;

/// Database-backed store for flow version tags.
pub struct SeaOrmTagStore {
    db: Arc<DatabaseConnection>,
}

impl SeaOrmTagStore {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

fn to_store_err(e: impl std::fmt::Display) -> FlowStoreError {
    FlowStoreError::Store {
        message: e.to_string(),
    }
}

fn model_to_tag(m: flow_tag::Model) -> FlowTag {
    FlowTag {
        flow_id: m.flow_id,
        tag: m.tag,
        version_id: m.version_id,
        updated_at: m.updated_at.and_utc(),
    }
}

#[async_trait]
impl TagStore for SeaOrmTagStore {
    async fn set_tag(
        &self,
        flow_id: &str,
        tag: &str,
        version_id: &str,
    ) -> Result<(), FlowStoreError> {
        let now = chrono::Utc::now().naive_utc();
        let model = flow_tag::ActiveModel {
            flow_id: ActiveValue::Set(flow_id.to_string()),
            tag: ActiveValue::Set(tag.to_string()),
            version_id: ActiveValue::Set(version_id.to_string()),
            updated_at: ActiveValue::Set(now),
        };

        let result = flow_tag::Entity::insert(model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    flow_tag::Column::FlowId,
                    flow_tag::Column::Tag,
                ])
                .update_columns([flow_tag::Column::VersionId, flow_tag::Column::UpdatedAt])
                .to_owned(),
            )
            .exec(self.db.as_ref())
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(to_store_err(e)),
        }
    }

    async fn get_tag(&self, flow_id: &str, tag: &str) -> Result<Option<String>, FlowStoreError> {
        let model = flow_tag::Entity::find()
            .filter(flow_tag::Column::FlowId.eq(flow_id))
            .filter(flow_tag::Column::Tag.eq(tag))
            .one(self.db.as_ref())
            .await
            .map_err(to_store_err)?;

        Ok(model.map(|m| m.version_id))
    }

    async fn list_tags(&self, flow_id: &str) -> Result<Vec<FlowTag>, FlowStoreError> {
        let models = flow_tag::Entity::find()
            .filter(flow_tag::Column::FlowId.eq(flow_id))
            .order_by_asc(flow_tag::Column::Tag)
            .all(self.db.as_ref())
            .await
            .map_err(to_store_err)?;

        Ok(models.into_iter().map(model_to_tag).collect())
    }

    async fn delete_tag(&self, flow_id: &str, tag: &str) -> Result<bool, FlowStoreError> {
        let result = flow_tag::Entity::delete_many()
            .filter(flow_tag::Column::FlowId.eq(flow_id))
            .filter(flow_tag::Column::Tag.eq(tag))
            .exec(self.db.as_ref())
            .await
            .map_err(to_store_err)?;

        Ok(result.rows_affected > 0)
    }

    async fn tags_for_version(&self, version_id: &str) -> Result<Vec<String>, FlowStoreError> {
        let models = flow_tag::Entity::find()
            .filter(flow_tag::Column::VersionId.eq(version_id))
            .order_by_asc(flow_tag::Column::Tag)
            .all(self.db.as_ref())
            .await
            .map_err(to_store_err)?;

        Ok(models.into_iter().map(|m| m.tag).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::MigratorTrait;

    async fn setup() -> (SeaOrmTagStore, Arc<DatabaseConnection>) {
        let db = Arc::new(
            sea_orm::Database::connect("sqlite::memory:")
                .await
                .expect("connect"),
        );
        crate::migrations::Migrator::up(db.as_ref(), None)
            .await
            .expect("migrate");
        let store = SeaOrmTagStore::new(Arc::clone(&db));
        (store, db)
    }

    #[tokio::test]
    async fn test_set_get_tag() {
        let (store, _db) = setup().await;
        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        let vid = store.get_tag("flow-1", "prod").await.unwrap();
        assert_eq!(vid, Some("v1".to_string()));
    }

    #[tokio::test]
    async fn test_upsert_tag() {
        let (store, _db) = setup().await;
        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        store.set_tag("flow-1", "prod", "v2").await.unwrap();
        let vid = store.get_tag("flow-1", "prod").await.unwrap();
        assert_eq!(vid, Some("v2".to_string()));
    }

    #[tokio::test]
    async fn test_list_tags() {
        let (store, _db) = setup().await;
        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        store.set_tag("flow-1", "staging", "v2").await.unwrap();
        let tags = store.list_tags("flow-1").await.unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tag, "prod");
        assert_eq!(tags[1].tag, "staging");
    }

    #[tokio::test]
    async fn test_delete_tag() {
        let (store, _db) = setup().await;
        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        assert!(store.delete_tag("flow-1", "prod").await.unwrap());
        assert!(!store.delete_tag("flow-1", "prod").await.unwrap());
    }

    #[tokio::test]
    async fn test_tags_for_version() {
        let (store, _db) = setup().await;
        store.set_tag("flow-1", "prod", "v1").await.unwrap();
        store.set_tag("flow-1", "latest", "v1").await.unwrap();
        store.set_tag("flow-2", "prod", "v1").await.unwrap();
        let tags = store.tags_for_version("v1").await.unwrap();
        assert_eq!(tags.len(), 3);
    }
}
