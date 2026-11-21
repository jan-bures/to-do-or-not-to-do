use std::io;
use std::path::PathBuf;

use crate::budget::OpClass;
use crate::store::StoreError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors from the backup library. None of them carries credentials, keys or database contents.
///
/// Each message already includes its cause, so the variants don't also expose it as `source()`: printing
/// the chain (anyhow's `{:#}`) would repeat it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),

    #[error("{context}: {error}")]
    Io { context: String, error: io::Error },

    #[error("sqlite: {0}")]
    Sqlite(rusqlite::Error),

    #[error("integrity check failed for {path}: {detail}")]
    Integrity { path: PathBuf, detail: String },

    #[error("encryption: {0}")]
    Crypto(String),

    #[error("object store: {0}")]
    Store(StoreError),

    #[error(
        "monthly budget for Class {class} operations reached in {month}: {used} used, {wanted} more needed, \
         cap {cap}. Refusing to run; raise the cap in the config only if you are sure it stays inside the \
         free tier"
    )]
    BudgetExceeded {
        class: OpClass,
        month: String,
        used: u64,
        wanted: u64,
        cap: u64,
    },

    #[error("refusing to overwrite existing file {0}")]
    OutputExists(PathBuf),

    #[error("no backup found: {0}")]
    NoBackup(String),

    #[error("another todo-backup process holds the lock on {0}")]
    Locked(PathBuf),
}

impl Error {
    pub(crate) fn io(context: impl Into<String>) -> impl FnOnce(io::Error) -> Error {
        let context = context.into();
        move |error| Error::Io { context, error }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Self {
        Error::Store(e)
    }
}
