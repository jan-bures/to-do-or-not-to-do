//! The TOML config and the JSON credentials file.
//!
//! Nothing deployment-specific has a default: the database path, state directory, recipient and credentials
//! file must all be given. Relative paths are resolved against the directory that holds the config file.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::budget::BudgetCaps;
use crate::{Error, Result};

/// `backup.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The live database, for example `<data_dir>/todo.sqlite3`.
    pub db_path: PathBuf,
    /// Holds the budget counters, the record of past uploads, the lock file and temporary snapshots. Keep it
    /// on the same ACL-restricted volume as the database: snapshots are plaintext until they are encrypted.
    pub state_dir: PathBuf,
    /// The age X25519 recipient (`age1...`) backups are encrypted to. Only the public half lives here.
    pub recipient: String,
    /// JSON file with `endpoint`, `bucket`, `access_key_id` and `secret_access_key`.
    pub credentials_file: PathBuf,
    #[serde(default)]
    pub budget: BudgetCaps,
    #[serde(default)]
    pub upload: UploadOptions,
    #[serde(default)]
    pub dedupe: DedupeOptions,
}

/// How long the bucket's lifecycle rules keep each kind. These mirror the rules the bucket must have; they
/// bound how long an unchanged database may go without a fresh upload.
pub const FREQUENT_RETENTION_HOURS: u64 = 7 * 24;
pub const DAILY_RETENTION_HOURS: u64 = 90 * 24;
/// A refreshed copy must land at least this long before the previous one could expire.
pub const RETENTION_MARGIN_HOURS: u64 = 24;

/// Skipping an unchanged upload is only allowed while the last upload of that kind is younger than this.
/// Otherwise an idle database would let every copy of a kind expire.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct DedupeOptions {
    pub frequent_max_age_hours: u64,
    pub daily_max_age_hours: u64,
}

impl Default for DedupeOptions {
    fn default() -> Self {
        Self {
            frequent_max_age_hours: 3 * 24,
            daily_max_age_hours: 30 * 24,
        }
    }
}

impl DedupeOptions {
    pub fn validate(&self) -> Result<()> {
        for (name, value, retention) in [
            (
                "frequent_max_age_hours",
                self.frequent_max_age_hours,
                FREQUENT_RETENTION_HOURS,
            ),
            (
                "daily_max_age_hours",
                self.daily_max_age_hours,
                DAILY_RETENTION_HOURS,
            ),
        ] {
            let limit = retention - RETENTION_MARGIN_HOURS;
            if value > limit {
                return Err(Error::Config(format!(
                    "dedupe.{name} = {value} is too close to the bucket's {retention} h retention; use at most {limit}"
                )));
            }
        }
        Ok(())
    }

    /// The longest an unchanged upload of `kind` may be relied on. Zero means always upload.
    pub fn max_age(&self, kind: crate::Kind) -> jiff::SignedDuration {
        let hours = match kind {
            crate::Kind::Frequent => self.frequent_max_age_hours,
            crate::Kind::Daily => self.daily_max_age_hours,
        };
        // Validated to be below the retention, so this can't overflow.
        jiff::SignedDuration::from_hours(i64::try_from(hours).unwrap_or(i64::MAX / 3600))
    }
}

/// Tuning for uploads. The defaults suit R2.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct UploadOptions {
    /// Objects up to this size go up in one PUT (one Class A operation).
    pub multipart_threshold_bytes: u64,
    /// Part size for larger objects. R2 needs at least 5 MiB for every part but the last.
    pub part_size_bytes: u64,
    /// zstd level. Higher is smaller and slower.
    pub zstd_level: i32,
}

impl Default for UploadOptions {
    fn default() -> Self {
        Self {
            multipart_threshold_bytes: 100 * 1024 * 1024,
            part_size_bytes: 32 * 1024 * 1024,
            zstd_level: 12,
        }
    }
}

impl Config {
    /// Reads and validates the config file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(Error::io(format!("reading config {}", path.display())))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Self::parse(&text, base)
    }

    /// Parses config text, resolving relative paths against `base`.
    pub fn parse(text: &str, base: &Path) -> Result<Self> {
        let mut cfg: Config =
            toml::from_str(text).map_err(|e| Error::Config(format!("invalid config: {e}")))?;
        for p in [
            &mut cfg.db_path,
            &mut cfg.state_dir,
            &mut cfg.credentials_file,
        ] {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
        cfg.recipient()?;
        cfg.budget.validate()?;
        cfg.dedupe.validate()?;
        if cfg.upload.part_size_bytes == 0 || cfg.upload.multipart_threshold_bytes == 0 {
            return Err(Error::Config(
                "upload sizes must be greater than zero".into(),
            ));
        }
        Ok(cfg)
    }

    pub fn recipient(&self) -> Result<age::x25519::Recipient> {
        self.recipient.trim().parse().map_err(|_| {
            Error::Config(
                "recipient is not an age X25519 public key (age1...); run `todo-backup keygen`"
                    .into(),
            )
        })
    }

    pub fn credentials(&self) -> Result<Credentials> {
        Credentials::load(&self.credentials_file)
    }
}

/// Bucket-scoped S3 credentials. `Debug` never prints the keys.
#[derive(Clone, Deserialize)]
pub struct Credentials {
    /// For R2: `https://<account-id>.r2.cloudflarestorage.com`.
    pub endpoint: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// R2 uses `auto`.
    #[serde(default = "default_region")]
    pub region: String,
}

fn default_region() -> String {
    "auto".into()
}

impl Credentials {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(Error::io(format!(
            "reading credentials file {}",
            path.display()
        )))?;
        // serde_json errors quote at most a short token, never the whole file, but keep only the position.
        serde_json::from_str(&text).map_err(|e| {
            Error::Config(format!(
                "credentials file {} is not valid (line {}, column {}); expected endpoint, bucket, \
                 access_key_id and secret_access_key",
                path.display(),
                e.line(),
                e.column()
            ))
        })
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("bucket", &self.bucket)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECIPIENT: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";

    #[test]
    fn parses_minimal_config_and_resolves_relative_paths() {
        let text = format!(
            "db_path = \"todo.sqlite3\"\nstate_dir = \"backup-state\"\nrecipient = \"{RECIPIENT}\"\n\
             credentials_file = \"r2.json\"\n"
        );
        let cfg = Config::parse(&text, Path::new("base")).expect("parse");
        assert_eq!(cfg.db_path, Path::new("base").join("todo.sqlite3"));
        assert_eq!(cfg.credentials_file, Path::new("base").join("r2.json"));
        assert_eq!(cfg.budget, BudgetCaps::default());
        assert_eq!(cfg.upload, UploadOptions::default());
        assert_eq!(cfg.dedupe.frequent_max_age_hours, 72);
        assert_eq!(cfg.dedupe.daily_max_age_hours, 720);
    }

    #[test]
    fn rejects_missing_fields_unknown_fields_and_bad_recipient() {
        assert!(Config::parse("db_path = \"x\"", Path::new(".")).is_err());
        let base = format!(
            "db_path = \"a\"\nstate_dir = \"b\"\ncredentials_file = \"c\"\nrecipient = \"{RECIPIENT}\"\n"
        );
        assert!(Config::parse(&format!("{base}typo = 1\n"), Path::new(".")).is_err());
        let bad = base.replace(RECIPIENT, "AGE-SECRET-KEY-1NOTAPUBLICKEY");
        let err = Config::parse(&bad, Path::new(".")).expect_err("secret key as recipient");
        // The error must not echo the value back.
        assert!(!err.to_string().contains("AGE-SECRET-KEY"));
    }

    #[test]
    fn dedupe_ages_must_stay_below_retention() {
        let base = format!(
            "db_path = \"a\"\nstate_dir = \"b\"\ncredentials_file = \"c\"\nrecipient = \"{RECIPIENT}\"\n"
        );
        let with =
            |extra: &str| Config::parse(&format!("{base}[dedupe]\n{extra}\n"), Path::new("."));
        with("frequent_max_age_hours = 144").expect("6 days is fine for a 7-day retention");
        assert!(with("frequent_max_age_hours = 145").is_err());
        assert!(with("frequent_max_age_hours = 168").is_err());
        with("daily_max_age_hours = 2136").expect("89 days is fine for a 90-day retention");
        assert!(with("daily_max_age_hours = 2160").is_err());
        with("frequent_max_age_hours = 0").expect("zero: always upload");
    }

    #[test]
    fn credentials_debug_is_redacted() {
        let c: Credentials = serde_json::from_str(
            r#"{"endpoint":"https://x.invalid","bucket":"b","access_key_id":"AKID","secret_access_key":"SECRET"}"#,
        )
        .expect("json");
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("AKID") && !dbg.contains("SECRET"));
        assert_eq!(c.region, "auto");
    }
}
