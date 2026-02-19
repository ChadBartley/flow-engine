//! Open-source storage layer for the ClearGate flow engine.
//!
//! Contains SeaORM entities, migrations, and store implementations for the
//! four flow-engine tables: `flow_head`, `flow_version`, `flow_tag`, and
//! `run_event`.

pub mod migrations;
pub mod models;
pub mod stores;

pub use stores::{SeaOrmFlowStore, SeaOrmRunStore, SeaOrmTagStore};

use sea_orm::{Database, DatabaseConnection, DbErr};
use sea_orm_migration::MigratorTrait;

/// Connect to a database using the given URL.
pub async fn connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    Database::connect(url).await
}

/// Run the OSS flow-engine migrations.
pub async fn run_migrations(db: &DatabaseConnection) -> Result<(), DbErr> {
    migrations::Migrator::up(db, None).await
}
