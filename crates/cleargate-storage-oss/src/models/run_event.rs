use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "run_event")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    #[sea_orm(column_type = "Json")]
    pub payload: serde_json::Value,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
