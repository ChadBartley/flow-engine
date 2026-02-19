pub use sea_orm_migration::prelude::*;

mod m20260215_000002_flow_engine_tables;
mod m20260216_000001_flow_tags;

pub use m20260215_000002_flow_engine_tables::Migration as FlowEngineTablesMigration;
pub use m20260216_000001_flow_tags::Migration as FlowTagsMigration;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260215_000002_flow_engine_tables::Migration),
            Box::new(m20260216_000001_flow_tags::Migration),
        ]
    }
}
