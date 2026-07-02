use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const IDX_PARENT_ID: &str = "idx_conversation_parent_id";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `parent_id` is queried as a filter predicate by `list_children`
        // (`parent_id = ?`) and by the `fill_child_counts` aggregate
        // (`parent_id IN (...) AND deleted_at IS NULL GROUP BY parent_id`) on
        // every sidebar list load once delegation produces sub-sessions.
        // Without this index SQLite falls back to a full table scan over
        // conversation, which grows linearly with session history.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name(IDX_PARENT_ID)
                    .table(Conversation::Table)
                    .col(Conversation::ParentId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .if_exists()
                    .name(IDX_PARENT_ID)
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    ParentId,
}
