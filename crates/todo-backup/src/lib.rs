//! Encrypted SQLite backups to S3-compatible storage (Cloudflare R2).
//!
//! A backup is a consistent snapshot of the live database (`VACUUM INTO`, safe while the server writes in WAL
//! mode), checked with `PRAGMA integrity_check`, compressed with zstd and encrypted with age to an X25519
//! recipient. The backup host only ever holds the public recipient; the identity that can decrypt lives in the
//! owner's password manager.
//!
//! Every S3 operation goes through [`store::Metered`], which charges a local monthly [`budget::Budget`] before
//! the request is sent and refuses once a cap is reached, so the tool can never run up a bill on its own.
//!
//! The binary (`src/main.rs`) is a thin CLI over [`ops`].

pub mod budget;
pub mod config;
pub mod crypto;
mod error;
pub mod ops;
pub mod snapshot;
pub mod state;
pub mod store;

pub use error::{Error, Result};
/// Owner-only files and durable replacement (the shared `todo-fs` crate).
pub use todo_fs as secure;

use std::fmt;
use std::str::FromStr;

/// Which schedule a backup belongs to. Each kind has its own key prefix, and the bucket's lifecycle rules expire
/// them differently (`frequent/` after 7 days, `daily/` after 90).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Frequent,
    Daily,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Frequent, Kind::Daily];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Frequent => "frequent",
            Kind::Daily => "daily",
        }
    }

    /// The key prefix, including the trailing slash.
    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Frequent => "frequent/",
            Kind::Daily => "daily/",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "frequent" => Ok(Kind::Frequent),
            "daily" => Ok(Kind::Daily),
            other => Err(Error::Config(format!(
                "unknown backup kind {other:?}; expected \"frequent\" or \"daily\""
            ))),
        }
    }
}
