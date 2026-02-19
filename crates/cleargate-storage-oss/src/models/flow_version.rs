use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "flow_version")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub version_id: String,
    pub flow_id: String,
    #[sea_orm(column_type = "Json")]
    pub graph: serde_json::Value,
    pub created_at: DateTime,
    pub created_by: Option<String>,
    pub message: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
