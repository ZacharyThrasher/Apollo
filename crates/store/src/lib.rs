//! SQLite store for non-secret application state (BUILD_SPEC STORE-01).
//!
//! The store never sees credentials, stream URLs or audio: its inputs are
//! domain types that cannot carry them. All calls are blocking; run them off
//! the UI thread (e.g. on a dedicated store thread or `spawn_blocking`).
//!
//! * Schema is versioned with `PRAGMA user_version` and migrated transactionally.
//! * A database written by a newer schema is refused, not touched.
//! * Before migrating an existing file database it is backed up (`VACUUM INTO`).
//! * The queue is saved in one transaction, so a crash can lose a few seconds
//!   of position but never leaves a half-written queue.
//! * The saved queue is bound to an opaque account key; a different account
//!   (or logout) purges it instead of replaying it.

mod blocked;
mod crate_list;
mod lyrics;
mod loudness;
mod migrations;
mod pages;
mod plays;
mod queue;

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

pub use crate_list::CrateRow;
pub use lyrics::CachedLyrics;
pub use loudness::LoudnessRow;
pub use migrations::{Migration, MIGRATIONS};
pub use pages::CachedPage;
pub use plays::{NewPlay, PlayTotal, MAX_PLAYS_PER_ACCOUNT};

#[derive(Debug)]
pub enum StoreError {
    /// Sanitized SQLite error text (no row data).
    Sqlite(String),
    /// The file was written by a newer version of the app.
    NewerSchema {
        found: i64,
        supported: i64,
    },
    /// Stored data failed validation; the caller may purge and start fresh.
    Corrupt(&'static str),
    /// A value does not fit the storage representation.
    OutOfRange(&'static str),
    Io(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "database error: {e}"),
            Self::NewerSchema { found, supported } => {
                write!(
                    f,
                    "database schema v{found} is newer than supported v{supported}"
                )
            }
            Self::Corrupt(what) => write!(f, "stored data is invalid: {what}"),
            Self::OutOfRange(what) => write!(f, "value out of range: {what}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        // Keep the error kind; rusqlite's Display can include SQL text but not row values.
        Self::Sqlite(e.to_string())
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_with(path, MIGRATIONS)
    }

    /// Open with an explicit migration list (tests inject failing migrations).
    #[doc(hidden)]
    pub fn open_with(path: &Path, migrations: &[Migration]) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        let mut store = Self { conn };
        store.configure()?;
        store.migrate(migrations, Some(path))?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.configure()?;
        store.migrate(MIGRATIONS, None)?;
        Ok(store)
    }

    fn configure(&self) -> Result<(), StoreError> {
        self.conn.busy_timeout(std::time::Duration::from_secs(5))?;
        self.conn.pragma_update(None, "foreign_keys", true)?;
        // WAL is a no-op for in-memory databases; ignore the returned mode.
        let _: String = self
            .conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        self.conn.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn migrate(&mut self, migrations: &[Migration], file: Option<&Path>) -> Result<(), StoreError> {
        let supported = migrations.last().map_or(0, |m| m.version);
        let current = self.schema_version()?;
        if current > supported {
            return Err(StoreError::NewerSchema {
                found: current,
                supported,
            });
        }
        if current == supported {
            return Ok(());
        }
        // Back up an existing database before changing its shape.
        if current > 0 {
            if let Some(path) = file {
                self.backup(path, current)?;
            }
        }
        for m in migrations.iter().filter(|m| m.version > current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(m.sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", m.version))?;
            tx.commit()?;
        }
        Ok(())
    }

    fn backup(&self, path: &Path, version: i64) -> Result<(), StoreError> {
        let mut target: PathBuf = path.to_path_buf();
        let name = format!(
            "{}.bak-v{version}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("apollo.sqlite")
        );
        target.set_file_name(name);
        if target.exists() {
            std::fs::remove_file(&target).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        let target = target
            .to_str()
            .ok_or(StoreError::Io("non-UTF-8 backup path".into()))?;
        self.conn.execute("VACUUM INTO ?1", [target])?;
        Ok(())
    }

    // ---- settings ------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        match self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            }) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
