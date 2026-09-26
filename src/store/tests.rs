use super::*;
use rusqlite::Connection;

fn config(name: &str) -> DatabaseConfig {
    DatabaseConfig {
        name: name.into(),
        ..Default::default()
    }
}
#[test]
fn enforces_project_names_database_names_ports_and_foreign_keys() {
    let store = Store::in_memory().unwrap();
    let project = store.create_project("Work").unwrap();
    assert!(store.create_project("work").is_err());
    let other = store.create_project("Other").unwrap();
    let db = store
        .insert_database(&project, config("api"), 5432)
        .unwrap();
    assert!(
        store
            .insert_database(&project, config("API"), 5433)
            .is_err()
    );
    assert!(store.insert_database(&other, config("api"), 5432).is_err());
    assert!(
        store
            .insert_database("missing", config("api"), 5440)
            .is_err()
    );
    assert!(store.delete_project(&project).is_err());
    store.delete_database(&db).unwrap();
    store.delete_project(&project).unwrap();
    store.insert_database(&other, config("api"), 5432).unwrap();
}
#[test]
fn reopens_and_preserves_credentials_and_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let (id, password) = {
        let store = Store::open(&path).unwrap();
        let project = store.create_project("App").unwrap();
        let id = store
            .insert_database(&project, config("api"), 5433)
            .unwrap();
        let password = store.database(&id).unwrap().password;
        (id, password)
    };
    let store = Store::open(&path).unwrap();
    let db = store.database(&id).unwrap();
    let credentials_match = db.password == password;
    assert!(credentials_match);
    assert_eq!(db.port, 5433);
    let mut changed = db.config;
    changed.tag = "17".into();
    assert!(store.update_database(&id, changed, 5433).is_err());
}

#[test]
fn typed_mutations_preserve_settings_and_enforce_update_constraints() {
    let store = Store::in_memory().unwrap();
    let project = store.create_project(" Zoo ").unwrap();
    let other = store.create_project("apple").unwrap();
    assert_eq!(store.projects().unwrap()[0].name, "apple");
    assert!(store.rename_project(&project, "APPLE").is_err());
    assert!(store.rename_project("missing", "Name").is_err());
    store.rename_project(&project, " Work ").unwrap();
    assert_eq!(store.projects().unwrap()[1].name, "Work");

    let id = store
        .insert_database(&project, config(" zebra "), 5432)
        .unwrap();
    let second = store
        .insert_database(&project, config("apple"), 5433)
        .unwrap();
    assert_eq!(store.databases().unwrap()[0].id, second);
    let original = store.database(&id).unwrap();
    let mut updated = original.config.clone();
    updated.name = " API ".into();
    updated.command = "-c wal_level=logical".into();
    updated.reserved_port = Some(5434);
    store.update_database(&id, updated.clone(), 5434).unwrap();
    updated.name = "API".into();
    let saved = store.database(&id).unwrap();
    assert_eq!(saved.config, updated);
    assert_eq!(saved.password, original.password);
    assert_eq!(saved.project_id, project);
    assert_eq!(saved.port, 5434);

    assert!(store.update_database(&id, config("APPLE"), 5434).is_err());
    assert!(store.update_database(&id, updated.clone(), 5433).is_err());
    assert!(store.update_database("missing", updated, 5434).is_err());
    assert_eq!(store.database(&id).unwrap().config, saved.config);
    assert_eq!(store.database(&id).unwrap().port, 5434);
    assert!(
        store
            .insert_database(&other, config("invalid port"), 1023)
            .is_err()
    );
    store.delete_database(&id).unwrap();
    assert!(store.database(&id).is_err());
    assert_eq!(store.allocated_ports().unwrap(), vec![5433]);
}

#[test]
fn individual_lookup_and_port_reservations_do_not_decode_unrelated_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace #100% café.sqlite3");
    let store = Store::open(&path).unwrap();
    let project = store.create_project("Work").unwrap();
    let good = store
        .insert_database(&project, config("good"), 5432)
        .unwrap();
    let bad = store
        .insert_database(&project, config("bad"), 5433)
        .unwrap();
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE databases SET config = '{}' WHERE id = ?1", [&bad])
        .unwrap();
    assert_eq!(store.database(&good).unwrap().config.name, "good");
    assert!(store.database(&bad).is_err());
    assert!(store.databases().is_err());
    let mut ports = store.allocated_ports().unwrap();
    ports.sort_unstable();
    assert_eq!(ports, vec![5432, 5433]);
}

#[test]
fn memory_workspaces_are_isolated() {
    let first = Store::in_memory().unwrap();
    first.create_project("Only here").unwrap();
    let second = Store::in_memory().unwrap();
    assert!(second.projects().unwrap().is_empty());
    assert_eq!(first.projects().unwrap().len(), 1);
}

#[test]
fn failed_initial_migration_rolls_back_schema_and_history_and_can_retry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("failed.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE databases (value TEXT);")
        .unwrap();
    assert!(Store::open(&path).is_err());
    assert!(!connection.table_exists(None, "projects").unwrap());
    assert!(!connection.table_exists(None, "seaql_migrations").unwrap());
    connection.execute_batch("DROP TABLE databases;").unwrap();
    let store = Store::open(&path).unwrap();
    let project = store.create_project("Recovered").unwrap();
    store
        .insert_database(&project, config("api"), 5432)
        .unwrap();
}

#[test]
fn unknown_migration_is_rejected_without_changing_data_or_history() {
    use sea_orm_migration::seaql_migrations;

    let store = Store::in_memory().unwrap();
    store.create_project("Preserved").unwrap();
    store.runtime.block_on(async {
        seaql_migrations::ActiveModel {
            version: Set("m20990101_000001_future".to_owned()),
            applied_at: Set(1),
        }
        .insert(&store.connection)
        .await
        .unwrap();
        assert!(migrations::migrate(&store.connection).await.is_err());
        assert_eq!(
            seaql_migrations::Entity::find()
                .all(&store.connection)
                .await
                .unwrap()
                .len(),
            2
        );
    });
    assert_eq!(store.projects().unwrap()[0].name, "Preserved");
}

#[test]
fn simultaneous_first_opens_apply_the_schema_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("concurrent.sqlite3");
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let open = || {
            barrier.wait();
            Store::open(&path).unwrap().projects().unwrap()
        };
        let first = scope.spawn(open);
        let second = scope.spawn(open);
        assert!(first.join().unwrap().is_empty());
        assert!(second.join().unwrap().is_empty());
    });
    let store = Store::open(&path).unwrap();
    store.create_project("Ready").unwrap();
}
