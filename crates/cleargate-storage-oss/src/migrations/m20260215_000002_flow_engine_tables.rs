use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. flow_heads
        manager
            .create_table(
                Table::create()
                    .table(FlowHead::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FlowHead::FlowId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FlowHead::Name).string().not_null())
                    .col(
                        ColumnDef::new(FlowHead::CurrentVersionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FlowHead::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // 2. flow_versions
        manager
            .create_table(
                Table::create()
                    .table(FlowVersion::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FlowVersion::VersionId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FlowVersion::FlowId).string().not_null())
                    .col(ColumnDef::new(FlowVersion::Graph).json().not_null())
                    .col(
                        ColumnDef::new(FlowVersion::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(FlowVersion::CreatedBy).string())
                    .col(ColumnDef::new(FlowVersion::Message).string())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_flow_versions_flow_id")
                    .table(FlowVersion::Table)
                    .col(FlowVersion::FlowId)
                    .to_owned(),
            )
            .await?;

        // 3. run_events
        manager
            .create_table(
                Table::create()
                    .table(RunEvent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RunEvent::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(RunEvent::RunId).string().not_null())
                    .col(ColumnDef::new(RunEvent::Seq).big_integer().not_null())
                    .col(ColumnDef::new(RunEvent::EventType).string().not_null())
                    .col(ColumnDef::new(RunEvent::Payload).json().not_null())
                    .col(
                        ColumnDef::new(RunEvent::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_run_events_run_id")
                    .table(RunEvent::Table)
                    .col(RunEvent::RunId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_run_events_run_id_seq")
                    .table(RunEvent::Table)
                    .col(RunEvent::RunId)
                    .col(RunEvent::Seq)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RunEvent::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(FlowVersion::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(FlowHead::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum FlowHead {
    Table,
    FlowId,
    Name,
    CurrentVersionId,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum FlowVersion {
    Table,
    VersionId,
    FlowId,
    Graph,
    CreatedAt,
    CreatedBy,
    Message,
}

#[derive(DeriveIden)]
enum RunEvent {
    Table,
    Id,
    RunId,
    Seq,
    EventType,
    Payload,
    CreatedAt,
}
