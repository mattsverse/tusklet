use crate::model::{Database, DatabaseConfig, Project, validate_name};
use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use rusqlite::{Connection, params};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

pub struct Store {
    connection: Connection,
}

/// Locate the default SQLite configuration file.
///
/// # Errors
/// Fails if the operating system cannot provide an application data directory.
pub fn default_path() -> Result<PathBuf> {
    Ok(ProjectDirs::from("dev", "tusklet", "Tusklet")
        .context("Could not find the application data directory.")?
        .data_local_dir()
        .join("tusklet.sqlite3"))
}

impl Store {
    /// Open or initialize a persistent workspace.
    ///
    /// # Errors
    /// Fails on directory, file permission, SQLite, or schema initialization errors.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        // Restrict the SQLite file before writing any connection credentials.
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// Open an isolated, non-persistent workspace.
    ///
    /// # Errors
    /// Fails if SQLite cannot initialize the database.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE UNIQUE
            );
            CREATE TABLE IF NOT EXISTS databases (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
                name TEXT NOT NULL COLLATE NOCASE, config TEXT NOT NULL,
                password TEXT NOT NULL, port INTEGER NOT NULL UNIQUE CHECK(port BETWEEN 1024 AND 65535),
                UNIQUE(project_id, name)
            );
            PRAGMA user_version = 1;")?;
        Ok(Self { connection })
    }

    /// List projects in name order.
    ///
    /// # Errors
    /// Fails if SQLite cannot read project records.
    pub fn projects(&self) -> Result<Vec<Project>> {
        Ok(self
            .connection
            .prepare("SELECT id, name FROM projects ORDER BY name COLLATE NOCASE")?
            .query_map([], |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Create a uniquely named project.
    ///
    /// # Errors
    /// Rejects invalid or duplicate names and propagates SQLite write failures.
    pub fn create_project(&self, name: &str) -> Result<String> {
        validate_name(name)?;
        let id = Uuid::new_v4().to_string();
        self.connection
            .execute(
                "INSERT INTO projects(id, name) VALUES (?1, ?2)",
                params![id, name.trim()],
            )
            .context("Could not create project. A project with that name may already exist.")?;
        Ok(id)
    }

    /// Rename an existing project.
    ///
    /// # Errors
    /// Rejects invalid or duplicate names, missing projects, and SQLite failures.
    pub fn rename_project(&self, id: &str, name: &str) -> Result<()> {
        validate_name(name)?;
        ensure!(
            self.connection
                .execute(
                    "UPDATE projects SET name=?2 WHERE id=?1",
                    params![id, name.trim()]
                )
                .context("A project with that name may already exist.")?
                == 1,
            "Project no longer exists."
        );
        Ok(())
    }

    /// Remove an empty project.
    ///
    /// # Errors
    /// Fails if the project contains databases or SQLite cannot delete it.
    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM projects WHERE id=?1", [id])
            .context("Remove the project's databases before deleting it.")?;
        Ok(())
    }

    /// Read all saved databases, including connection credentials.
    ///
    /// # Errors
    /// Fails on SQLite read errors or invalid saved configuration.
    pub fn databases(&self) -> Result<Vec<Database>> {
        let mut statement = self.connection.prepare("SELECT id, project_id, config, password, port FROM databases ORDER BY name COLLATE NOCASE")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u16>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, project_id, config, password, port) = row?;
            Ok(Database {
                id,
                project_id,
                config: serde_json::from_str(&config)?,
                password,
                port,
            })
        })
        .collect()
    }

    /// Find a saved database by its stable identifier.
    ///
    /// # Errors
    /// Fails on read errors or when the database no longer exists.
    pub fn database(&self, id: &str) -> Result<Database> {
        self.databases()?
            .into_iter()
            .find(|db| db.id == id)
            .context("Database no longer exists.")
    }

    /// Read ports reserved by all saved databases, including stopped ones.
    ///
    /// # Errors
    /// Fails if persisted database records cannot be read.
    pub fn allocated_ports(&self) -> Result<Vec<u16>> {
        Ok(self.databases()?.iter().map(|db| db.port).collect())
    }

    /// Persist a validated database with random connection credentials.
    ///
    /// # Errors
    /// Rejects invalid settings, missing projects, duplicate names, reserved
    /// ports, and SQLite failures.
    pub fn insert_database(
        &self,
        project_id: &str,
        mut config: DatabaseConfig,
        port: u16,
    ) -> Result<String> {
        config.name = config.name.trim().into();
        config.validate()?;
        let id = Uuid::new_v4().to_string();
        // Random per-database development credentials; never a shared default password.
        let password = Uuid::new_v4().simple().to_string();
        self.connection.execute("INSERT INTO databases(id, project_id, name, config, password, port) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, project_id, config.name, serde_json::to_string(&config)?, password, port])
            .context("Could not save database. Check that the project exists, the name is unique, and the port is not reserved.")?;
        Ok(id)
    }

    /// Update mutable configuration without changing database identity.
    ///
    /// # Errors
    /// Rejects immutable-field changes, invalid settings, missing databases,
    /// conflicting names or ports, and SQLite failures.
    pub fn update_database(&self, id: &str, mut config: DatabaseConfig, port: u16) -> Result<()> {
        config.name = config.name.trim().into();
        config.validate()?;
        let original = self.database(id)?;
        ensure!(
            config.tag == original.config.tag
                && config.database == original.config.database
                && config.user == original.config.user,
            "Image tag, initial database, and user cannot change on an existing database. Create a new database and use dump/restore to migrate."
        );
        self.connection
            .execute(
                "UPDATE databases SET name=?2, config=?3, port=?4 WHERE id=?1",
                params![id, config.name, serde_json::to_string(&config)?, port],
            )
            .context("Could not save settings. The database name or port may already be in use.")?;
        Ok(())
    }

    /// Remove the configuration record after container removal succeeds.
    ///
    /// # Errors
    /// Fails if SQLite cannot delete the record.
    pub fn delete_database(&self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM databases WHERE id=?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
