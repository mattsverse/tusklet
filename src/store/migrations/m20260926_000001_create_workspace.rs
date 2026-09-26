use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &'static str {
        "m20260926_000001_create_workspace"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Projects::Table)
                    .col(ColumnDef::new(Projects::Id).text().not_null().primary_key())
                    .col(
                        ColumnDef::new(Projects::Name)
                            .text()
                            .not_null()
                            .unique_key()
                            // SeaQuery has no column-collation builder.
                            .extra("COLLATE NOCASE"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Databases::Table)
                    .col(
                        ColumnDef::new(Databases::Id)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Databases::ProjectId).text().not_null())
                    .col(
                        ColumnDef::new(Databases::Name)
                            .text()
                            .not_null()
                            .extra("COLLATE NOCASE"),
                    )
                    .col(ColumnDef::new(Databases::Config).text().not_null())
                    .col(ColumnDef::new(Databases::Password).text().not_null())
                    .col(
                        ColumnDef::new(Databases::Port)
                            .integer()
                            .not_null()
                            .unique_key()
                            .check(Expr::col(Databases::Port).between(1024, 65535)),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(Databases::ProjectId)
                            .col(Databases::Name),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Databases::Table, Databases::ProjectId)
                            .to(Projects::Table, Projects::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Databases::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Projects::Table).to_owned())
            .await
    }
}

// Migration-local identifiers must not change when the current entities evolve.
#[derive(DeriveIden)]
enum Projects {
    Table,
    Id,
    Name,
}

#[derive(DeriveIden)]
enum Databases {
    Table,
    Id,
    ProjectId,
    Name,
    Config,
    Password,
    Port,
}
