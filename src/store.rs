use crate::model::{Database, DatabaseConfig, Project, validate_name};
use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use sea_orm::{
    ActiveModelTrait, ConnectOptions, DatabaseConnection, EntityTrait, IntoActiveModel, QueryOrder,
    QuerySelect, Set,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::runtime::{Builder, Runtime};
use uuid::Uuid;

mod entities;
mod migrations;
#[cfg(test)]
mod tests;

use entities::{database, project};

/// Synchronous persistence facade used by the service's worker thread.
pub struct Store {
    connection: DatabaseConnection,
    runtime: Runtime,
}

/// Locate the default SQLite configuration file.
///
/// # Errors
/// Fails if the operating system cannot provide an application data directory.
pub fn default_path() -> Result<PathBuf> {
    Ok(ProjectDirs::from("com", "matteogassend", "tusklet")
        .context("Could not find the application data directory.")?
        .data_local_dir()
        .join("tusklet.sqlite3"))
}

impl Store {
    /// Open or initialize a persistent workspace.
    ///
    /// # Errors
    /// Fails on directory, file permission, SQLite, or schema migration errors.
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
        Self::connect(Some(path.to_path_buf()))
    }

    /// Open an isolated, non-persistent workspace.
    ///
    /// # Errors
    /// Fails if SQLite cannot initialize the database.
    pub fn in_memory() -> Result<Self> {
        Self::connect(None)
    }

    fn connect(path: Option<PathBuf>) -> Result<Self> {
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let mut options = ConnectOptions::new("sqlite::memory:");
        options
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            // Query parameters include local database passwords.
            .sqlx_logging(false)
            .map_sqlx_sqlite_opts(move |options| {
                let options = options
                    .foreign_keys(true)
                    .shared_cache(false)
                    .busy_timeout(Duration::from_secs(5));
                match &path {
                    Some(path) => options.in_memory(false).filename(path),
                    None => options,
                }
            });
        let connection = runtime.block_on(sea_orm::Database::connect(options))?;
        let store = Self {
            connection,
            runtime,
        };
        store
            .runtime
            .block_on(migrations::migrate(&store.connection))
            .context("Could not migrate the workspace database schema.")?;
        Ok(store)
    }

    /// List projects in name order.
    ///
    /// # Errors
    /// Fails if SQLite cannot read project records.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let models = self.runtime.block_on(
            project::Entity::find()
                .order_by_asc(project::COLUMN.name)
                .all(&self.connection),
        )?;
        Ok(models
            .into_iter()
            .map(|model| Project {
                id: model.id,
                name: model.name,
            })
            .collect())
    }

    /// Create a uniquely named project.
    ///
    /// # Errors
    /// Rejects invalid or duplicate names and propagates SQLite write failures.
    pub fn create_project(&self, name: &str) -> Result<String> {
        validate_name(name)?;
        let id = Uuid::new_v4().to_string();
        self.runtime
            .block_on(
                project::ActiveModel {
                    id: Set(id.clone()),
                    name: Set(name.trim().to_owned()),
                }
                .insert(&self.connection),
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
        self.runtime.block_on(async {
            let mut model = project::Entity::find_by_id(id)
                .one(&self.connection)
                .await?
                .context("Project no longer exists.")?
                .into_active_model();
            model.name = Set(name.trim().to_owned());
            model
                .update(&self.connection)
                .await
                .context("A project with that name may already exist.")?;
            Ok(())
        })
    }

    /// Remove an empty project.
    ///
    /// # Errors
    /// Fails if the project contains databases or SQLite cannot delete it.
    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.runtime
            .block_on(project::Entity::delete_by_id(id).exec(&self.connection))
            .context("Remove the project's databases before deleting it.")?;
        Ok(())
    }

    /// Read all saved databases, including connection credentials.
    ///
    /// # Errors
    /// Fails on SQLite read errors or invalid saved configuration.
    pub fn databases(&self) -> Result<Vec<Database>> {
        let models = self.runtime.block_on(
            database::Entity::find()
                .order_by_asc(database::COLUMN.name)
                .all(&self.connection),
        )?;
        Ok(models.into_iter().map(Database::from).collect())
    }

    /// Find a saved database by its stable identifier.
    ///
    /// # Errors
    /// Fails on read errors or when the database no longer exists.
    pub fn database(&self, id: &str) -> Result<Database> {
        self.runtime
            .block_on(database::Entity::find_by_id(id).one(&self.connection))?
            .map(Database::from)
            .context("Database no longer exists.")
    }

    /// Read ports reserved by all saved databases, including stopped ones.
    ///
    /// # Errors
    /// Fails if persisted ports cannot be read.
    pub fn allocated_ports(&self) -> Result<Vec<u16>> {
        Ok(self.runtime.block_on(
            database::Entity::find()
                .select_only()
                .column(database::Column::Port)
                .into_tuple::<u16>()
                .all(&self.connection),
        )?)
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
        self.runtime
            .block_on(database::ActiveModel {
                id: Set(id.clone()),
                project_id: Set(project_id.to_owned()),
                name: Set(config.name.clone()),
                config: Set(config),
                password: Set(password),
                port: Set(port),
            }.insert(&self.connection))
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
        self.runtime.block_on(async {
            let original = database::Entity::find_by_id(id)
                .one(&self.connection)
                .await?
                .context("Database no longer exists.")?;
            ensure!(
                config.tag == original.config.tag
                    && config.database == original.config.database
                    && config.user == original.config.user,
                "Image tag, initial database, and user cannot change on an existing database. Create a new database and use dump/restore to migrate."
            );
            let mut model = original.into_active_model();
            model.name = Set(config.name.clone());
            model.config = Set(config);
            model.port = Set(port);
            model.update(&self.connection).await
                .context("Could not save settings. The database name or port may already be in use.")?;
            Ok(())
        })
    }

    /// Remove the configuration record after container removal succeeds.
    ///
    /// # Errors
    /// Fails if SQLite cannot delete the record.
    pub fn delete_database(&self, id: &str) -> Result<()> {
        self.runtime
            .block_on(database::Entity::delete_by_id(id).exec(&self.connection))?;
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Close the pool while its runtime is still alive, releasing SQLite locks.
        let _ = self.runtime.block_on(self.connection.close_by_ref());
    }
}

impl From<database::Model> for Database {
    fn from(model: database::Model) -> Self {
        Self {
            id: model.id,
            project_id: model.project_id,
            config: model.config,
            password: model.password,
            port: model.port,
        }
    }
}
