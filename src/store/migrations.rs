use sea_orm::{DatabaseConnection, SqliteTransactionMode, TransactionOptions, TransactionTrait};
use sea_orm_migration::prelude::*;

mod m20260926_000001_create_workspace;

pub(super) struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        // Append new migrations; keep released migrations and their identifiers frozen.
        vec![Box::new(m20260926_000001_create_workspace::Migration)]
    }
}

pub(super) async fn migrate(connection: &DatabaseConnection) -> Result<(), DbErr> {
    run::<Migrator>(connection).await
}

async fn run<M: MigratorTrait>(connection: &DatabaseConnection) -> Result<(), DbErr> {
    // SQLite migrations are not automatically transactional in SeaORM. Lock before
    // reading migration history so concurrent startups cannot apply the same step.
    let transaction = connection
        .begin_with_options(TransactionOptions {
            sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
            ..Default::default()
        })
        .await?;
    match M::up(&transaction, None).await {
        Ok(()) => transaction.commit().await,
        Err(error) => {
            transaction.rollback().await?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[derive(DeriveMigrationName)]
    struct AddSortOrder;

    #[async_trait::async_trait]
    impl MigrationTrait for AddSortOrder {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("projects"))
                        .add_column(
                            ColumnDef::new(Alias::new("sort_order"))
                                .integer()
                                .not_null()
                                .default(0),
                        )
                        .to_owned(),
                )
                .await
        }
    }

    struct Upgrade;

    #[async_trait::async_trait]
    impl MigratorTrait for Upgrade {
        fn migrations() -> Vec<Box<dyn MigrationTrait>> {
            let mut steps = Migrator::migrations();
            steps.push(Box::new(AddSortOrder));
            steps
        }
    }

    struct FailingStep;

    impl MigrationName for FailingStep {
        fn name(&self) -> &'static str {
            "test_failing_step"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for FailingStep {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            // Creating the existing projects table must fail after AddSortOrder succeeds.
            m20260926_000001_create_workspace::Migration
                .up(manager)
                .await
        }
    }

    struct FailingUpgrade;

    #[async_trait::async_trait]
    impl MigratorTrait for FailingUpgrade {
        fn migrations() -> Vec<Box<dyn MigrationTrait>> {
            let mut steps = Upgrade::migrations();
            steps.push(Box::new(FailingStep));
            steps
        }
    }

    #[test]
    fn pending_migrations_roll_back_together_and_only_apply_once_after_retry() {
        let store = Store::in_memory().unwrap();
        let id = store.create_project("Preserved").unwrap();
        store.runtime.block_on(async {
            assert!(run::<FailingUpgrade>(&store.connection).await.is_err());
            let manager = SchemaManager::new(&store.connection);
            assert!(!manager.has_column("projects", "sort_order").await.unwrap());
            assert_eq!(
                Migrator::get_applied_migrations(&store.connection)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            run::<Upgrade>(&store.connection).await.unwrap();
            run::<Upgrade>(&store.connection).await.unwrap();
            assert!(manager.has_column("projects", "sort_order").await.unwrap());
            assert_eq!(
                Upgrade::get_applied_migrations(&store.connection)
                    .await
                    .unwrap()
                    .len(),
                2
            );
        });
        let projects = store.projects().unwrap();
        assert_eq!(projects[0].id, id);
        assert_eq!(projects[0].name, "Preserved");
    }

    #[test]
    fn initial_migration_can_be_reversed_and_reapplied() {
        let store = Store::in_memory().unwrap();
        store.runtime.block_on(async {
            let transaction = store.connection.begin().await.unwrap();
            Migrator::down(&transaction, None).await.unwrap();
            transaction.commit().await.unwrap();
            let manager = SchemaManager::new(&store.connection);
            assert!(!manager.has_table("projects").await.unwrap());
            assert!(!manager.has_table("databases").await.unwrap());
            migrate(&store.connection).await.unwrap();
        });
        assert!(store.projects().unwrap().is_empty());
        store.create_project("Recreated").unwrap();
    }
}
