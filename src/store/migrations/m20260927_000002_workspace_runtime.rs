use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Workspace::Table)
                    .col(
                        ColumnDef::new(Workspace::Id)
                            .integer()
                            .not_null()
                            .primary_key()
                            .check(Expr::col(Workspace::Id).eq(1)),
                    )
                    .col(ColumnDef::new(Workspace::Runtime).text().not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Workspace::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Workspace {
    Table,
    Id,
    Runtime,
}
