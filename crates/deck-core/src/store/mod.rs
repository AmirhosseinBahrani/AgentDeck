//! SQLite persistence.
//!
//! Two pools by design: a **single-connection writer** and a multi-connection reader.
//! One writer makes `SQLITE_BUSY` structurally impossible rather than something to retry
//! under load, which matters because the event writer commits continuously while the UI
//! reads concurrently.

pub mod agents;
pub mod events;
pub mod identity;
pub mod processes;
pub mod runs;
pub mod sessions;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

#[derive(Clone)]
pub struct Store {
    writer: Pool<Sqlite>,
    reader: Pool<Sqlite>,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let url = format!("sqlite://{}", path.as_ref().display());
        Self::connect(&url, true).await
    }

    /// In-memory store for tests.
    ///
    /// `cache=shared` is required so the writer and reader pools see the same database,
    /// which in-memory SQLite otherwise would not do. The name is randomized per call
    /// because `cache=shared` is keyed by name — a fixed name would make concurrently
    /// running tests share one database and race on migration.
    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let name = uuid::Uuid::new_v4();
        Self::connect(
            &format!("sqlite:file:agentdeck_{name}?mode=memory&cache=shared"),
            false,
        )
        .await
    }

    async fn connect(url: &str, create: bool) -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(create)
            .foreign_keys(true)
            // WAL lets readers proceed during writes. NORMAL is the right durability
            // trade for a local event log: a crash can lose the last commit, and the
            // reconcile() boot step already assumes in-flight state is untrustworthy.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts.clone())
            .await?;

        MIGRATOR.run(&writer).await?;

        let reader = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts.read_only(false))
            .await?;

        Ok(Self { writer, reader })
    }

    /// Every mutation goes through here, so writes are serialized by construction.
    pub fn writer(&self) -> &Pool<Sqlite> {
        &self.writer
    }

    pub fn reader(&self) -> &Pool<Sqlite> {
        &self.reader
    }

    pub async fn close(&self) {
        self.writer.close().await;
        self.reader.close().await;
    }
}
