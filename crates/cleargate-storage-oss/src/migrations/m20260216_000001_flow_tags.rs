use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FlowTag::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(FlowTag::FlowId).string().not_null())
                    .col(ColumnDef::new(FlowTag::Tag).string().not_null())
                    .col(ColumnDef::new(FlowTag::VersionId).string().not_null())
                    .col(
                        ColumnDef::new(FlowTag::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(Index::create().col(FlowTag::FlowId).col(FlowTag::Tag))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_flow_tag_version_id")
                    .table(FlowTag::Table)
                    .col(FlowTag::VersionId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FlowTag::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum FlowTag {
    Table,
    FlowId,
    Tag,
    VersionId,
    UpdatedAt,
}
