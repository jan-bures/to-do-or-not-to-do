//! Where the app keeps its pairing and its data, outside the repo and per user:
//!
//! | File | Where | Contents |
//! |---|---|---|
//! | `config.json` | `%APPDATA%\to-do-or-not-to-do\` | the server URL learnt at pairing, the device ID and a random pairing ID |
//! | `device-token.bin` | same | the device token, encrypted with DPAPI for the current user |
//! | `pairings\<pairing ID>\cache.sqlite` | `%LOCALAPPDATA%\to-do-or-not-to-do\` | the sync engine's replica and outbox |
//! | `pairings\<pairing ID>\attachments\` | same | attachments downloaded to be opened |
//!
//! `TODO_DESKTOP_HOME` puts everything in one directory instead (tests, benchmarks, a second profile).
//! The token is never written in plain text and never logged.
//!
//! A pairing's data lives in a directory named after that pairing, so a new pairing never picks up an
//! old cache, and cleaning up after an old pairing can't touch the current one. Signing out removes the
//! token and the config, marks the pairing's directory as signed out and removes it; a directory that
//! couldn't be removed then (a file still open) is removed at the next start.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use todo_client::Entropy as _;
use todo_core::DeviceId;

use crate::platform::dpapi;

const APP_DIR: &str = "to-do-or-not-to-do";
const CONFIG_FILE: &str = "config.json";
const TOKEN_FILE: &str = "device-token.bin";
const PAIRINGS_DIR: &str = "pairings";
const CACHE_FILE: &str = "cache.sqlite";
const ATTACHMENTS_DIR: &str = "attachments";
/// Left in a pairing's directory once its sign-in is gone: the directory may be removed.
const SIGNED_OUT: &str = "signed-out";

/// The app's directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// Roaming settings: config and the sealed token.
    pub config_dir: PathBuf,
    /// Machine-local data: one directory per pairing.
    pub cache_dir: PathBuf,
}

impl Paths {
    /// `%APPDATA%` and `%LOCALAPPDATA%`, or `TODO_DESKTOP_HOME` for both.
    pub fn for_user() -> anyhow::Result<Self> {
        if let Some(home) = std::env::var_os("TODO_DESKTOP_HOME").filter(|v| !v.is_empty()) {
            return Ok(Self::in_dir(PathBuf::from(home)));
        }
        let dir = |var: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(|v| PathBuf::from(v).join(APP_DIR))
                .ok_or_else(|| anyhow::anyhow!("{var} is not set"))
        };
        Ok(Self {
            config_dir: dir("APPDATA")?,
            cache_dir: dir("LOCALAPPDATA")?,
        })
    }

    /// Everything in one directory.
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            config_dir: dir.clone(),
            cache_dir: dir,
        }
    }

    pub fn config(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE)
    }

    pub fn token(&self) -> PathBuf {
        self.config_dir.join(TOKEN_FILE)
    }

    fn pairings(&self) -> PathBuf {
        self.cache_dir.join(PAIRINGS_DIR)
    }

    /// The directory holding one pairing's data.
    pub fn data(&self, pairing: &PairingId) -> DataDir {
        DataDir(self.pairings().join(pairing.as_str()))
    }
}

/// One pairing's data directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataDir(PathBuf);

impl DataDir {
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn cache(&self) -> PathBuf {
        self.0.join(CACHE_FILE)
    }

    /// Where attachments are downloaded to be opened.
    pub fn attachments(&self) -> PathBuf {
        self.0.join(ATTACHMENTS_DIR)
    }
}

/// A random ID for one pairing: 32 lowercase hex digits, safe as a directory name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PairingId(String);

impl PairingId {
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        todo_client::native::OsEntropy.fill(&mut bytes);
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn valid(s: &str) -> bool {
        s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
}

impl TryFrom<String> for PairingId {
    type Error = &'static str;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::valid(&s)
            .then_some(Self(s))
            .ok_or("a pairing ID is 32 lowercase hex digits")
    }
}

impl From<PairingId> for String {
    fn from(id: PairingId) -> Self {
        id.0
    }
}

/// What pairing taught the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The server's base URL, for example `https://todo.example.com`.
    pub server: String,
    /// This device's ID: its changes are recorded as `owner/<device>`.
    pub device: DeviceId,
    /// Names this pairing's data directory.
    pub pairing: PairingId,
}

impl Config {
    /// A new pairing's config, with a fresh pairing ID.
    pub fn new(server: impl Into<String>, device: DeviceId) -> Self {
        Self {
            server: server.into(),
            device,
            pairing: PairingId::random(),
        }
    }
}

/// A paired device: its config and token.
#[derive(Clone, PartialEq, Eq)]
pub struct Pairing {
    pub config: Config,
    pub token: String,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("Pairing")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Writes `bytes` to `path` through a temporary file, so a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The saved pairing, if there is a complete one. A config without a token (or the reverse) counts as
/// not paired.
pub fn load(paths: &Paths) -> anyhow::Result<Option<Pairing>> {
    let config = match std::fs::read(paths.config()) {
        Ok(bytes) => serde_json::from_slice::<Config>(&bytes)?,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let sealed = match std::fs::read(paths.token()) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let token = String::from_utf8(dpapi::unprotect(&sealed)?)
        .map_err(|_| anyhow::anyhow!("the stored device token is not text"))?;
    Ok(Some(Pairing { config, token }))
}

/// The saved server URL, even without a token (to prefill the pairing screen).
pub fn last_server(paths: &Paths) -> Option<String> {
    let bytes = std::fs::read(paths.config()).ok()?;
    serde_json::from_slice::<Config>(&bytes)
        .ok()
        .map(|c| c.server)
}

/// Saves a pairing: the token sealed with DPAPI, then the config.
pub fn save(paths: &Paths, pairing: &Pairing) -> anyhow::Result<()> {
    let sealed = dpapi::protect(pairing.token.as_bytes())?;
    write_atomic(&paths.token(), &sealed)?;
    write_atomic(
        &paths.config(),
        &serde_json::to_vec_pretty(&pairing.config)?,
    )?;
    Ok(())
}

/// Removes the token and the config: the device is signed out from here on. The token goes first, so
/// a failure halfway leaves no usable sign-in.
pub fn forget_pairing(paths: &Paths) -> anyhow::Result<()> {
    remove(&paths.token())?;
    remove(&paths.config())?;
    Ok(())
}

/// Marks a pairing's data as signed out, so it is removed even if removing it now fails.
pub fn mark_signed_out(data: &DataDir) -> std::io::Result<()> {
    match std::fs::write(data.path().join(SIGNED_OUT), b"") {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()), // No data at all.
        other => other,
    }
}

/// Removes a pairing's data directory: the cache and the downloaded attachments. Fails while a session
/// still has the cache open.
pub fn remove_data(data: &DataDir) -> std::io::Result<()> {
    match std::fs::remove_dir_all(data.path()) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes the data of pairings that were signed out but couldn't be cleaned up then. Only directories
/// carrying the signed-out mark go, never `keep` (the current pairing). Returns how many were removed.
pub fn sweep_signed_out(paths: &Paths, keep: Option<&PairingId>) -> usize {
    let Ok(entries) = std::fs::read_dir(paths.pairings()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let current = keep.is_some_and(|k| name.to_str() == Some(k.as_str()));
        if current || !path.join(SIGNED_OUT).exists() {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!("signed-out data couldn't be removed yet: {e}"),
        }
    }
    removed
}

/// The name the owner sees when approving: "To-do on <computer name>" (read at run time, never built
/// in), within the server's 64 characters.
pub fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty() && !h.chars().any(char::is_control));
    let name = match host {
        Some(h) => format!("To-do on {h}"),
        None => "To-do for Windows".to_string(),
    };
    name.chars().take(64).collect()
}

/// Normalizes a typed server address: trims it and adds `https://` when no scheme is given, then checks
/// it with the transport's own parser (`todo_client::native::server_origin`), so the address saved here
/// is exactly the one the transport accepts. Plain `http://` is only accepted for this machine
/// (development servers), so the token never crosses a network unencrypted. No user name, query or
/// fragment.
pub fn normalize_server(input: &str) -> Result<String, String> {
    use todo_client::native::{BuildError, server_origin};
    let s = input.trim();
    if s.is_empty() {
        return Err("Enter your server's address.".into());
    }
    let url = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    server_origin(&url).map_err(|e| match e {
        BuildError::Insecure => "Use https:// (plain http only works for this computer).".into(),
        other => sentence(&other.to_string()),
    })
}

/// "the address …" → "The address …."
fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    out.extend(chars);
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> Paths {
        let dir = std::env::temp_dir().join(format!(
            "todo-desktop-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        Paths::in_dir(dir)
    }

    fn pairing() -> Pairing {
        Pairing {
            config: Config::new(
                "https://todo.example.com",
                DeviceId::new("dev-1").expect("id"),
            ),
            token: "s3cret-device-token-0123456789".into(),
        }
    }

    #[test]
    fn a_pairing_round_trips_with_the_token_sealed() {
        let paths = temp();
        assert_eq!(load(&paths).expect("load"), None);
        let pairing = pairing();
        save(&paths, &pairing).expect("save");
        assert_eq!(load(&paths).expect("load"), Some(pairing.clone()));
        // The token isn't on disk in plain text, anywhere.
        for entry in std::fs::read_dir(&paths.config_dir).expect("dir") {
            let bytes = std::fs::read(entry.expect("entry").path()).expect("read");
            assert!(!bytes.windows(6).any(|w| w == b"s3cret"));
        }
        assert!(!format!("{pairing:?}").contains("s3cret"));
        assert_eq!(
            last_server(&paths).as_deref(),
            Some("https://todo.example.com")
        );

        forget_pairing(&paths).expect("forget");
        assert_eq!(load(&paths).expect("load"), None);
        assert_eq!(last_server(&paths), None);
        forget_pairing(&paths).expect("forgetting twice is fine");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    #[test]
    fn each_pairing_has_its_own_data_and_cleanup_only_takes_signed_out_ones() {
        let paths = temp();
        let (old, new) = (pairing(), pairing());
        assert_ne!(old.config.pairing, new.config.pairing);
        let (old_data, new_data) = (
            paths.data(&old.config.pairing),
            paths.data(&new.config.pairing),
        );
        assert_ne!(old_data.cache(), new_data.cache());
        for d in [&old_data, &new_data] {
            std::fs::create_dir_all(d.attachments()).expect("dir");
            std::fs::write(d.cache(), b"cache").expect("cache");
        }
        // Not signed out: never swept.
        assert_eq!(sweep_signed_out(&paths, None), 0);
        mark_signed_out(&old_data).expect("mark");
        // The current pairing is kept even if marked.
        assert_eq!(sweep_signed_out(&paths, Some(&old.config.pairing)), 0);
        assert_eq!(sweep_signed_out(&paths, Some(&new.config.pairing)), 1);
        assert!(!old_data.path().exists());
        assert!(new_data.cache().exists());
        remove_data(&new_data).expect("remove");
        remove_data(&new_data).expect("removing twice is fine");
        mark_signed_out(&new_data).expect("marking missing data is fine");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    #[test]
    fn a_pairing_id_is_only_ever_hex() {
        let id = PairingId::random();
        assert!(PairingId::valid(id.as_str()));
        for bad in ["..", "../x", "ABCDEF0123456789ABCDEF0123456789", "", "0"] {
            assert!(PairingId::try_from(bad.to_string()).is_err(), "{bad}");
        }
        let json = r#"{"server":"https://todo.example.com","device":"d","pairing":"..\\x"}"#;
        assert!(serde_json::from_str::<Config>(json).is_err());
    }

    #[test]
    fn a_token_without_config_is_not_a_pairing() {
        let paths = temp();
        std::fs::create_dir_all(&paths.config_dir).expect("dir");
        std::fs::write(paths.token(), dpapi::protect(b"x").expect("seal")).expect("write");
        assert_eq!(load(&paths).expect("load"), None);
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    #[test]
    fn server_addresses_are_normalized_and_plain_http_stays_local() {
        assert_eq!(
            normalize_server(" todo.example.com/ ").as_deref(),
            Ok("https://todo.example.com")
        );
        assert_eq!(
            normalize_server("https://todo.example.com:8443").as_deref(),
            Ok("https://todo.example.com:8443")
        );
        assert_eq!(
            normalize_server("http://127.0.0.1:8080").as_deref(),
            Ok("http://127.0.0.1:8080")
        );
        assert_eq!(
            normalize_server("http://localhost:3000/").as_deref(),
            Ok("http://localhost:3000")
        );
        assert!(normalize_server("http://todo.example.com").is_err());
        assert!(normalize_server("http://localhost.example.com").is_err());
        assert!(normalize_server("ftp://todo.example.com").is_err());
        assert!(normalize_server("").is_err());
        assert!(normalize_server("https://").is_err());
        // One parser for the host: what's saved is what the transport will talk to.
        assert_eq!(
            normalize_server("TODO.Example.com").as_deref(),
            Ok("https://todo.example.com")
        );
        for bad in [
            "https://todo.example.com@evil.example",
            "https://user:pass@todo.example.com",
            "https://todo.example.com/?next=https://evil.example",
            "https://todo.example.com/#evil",
            "javascript:alert(1)",
            "file:///C:/Windows",
        ] {
            assert!(normalize_server(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_device_name_fits() {
        let n = device_name();
        assert!(n.starts_with("To-do"));
        assert!(n.chars().count() <= 64);
    }
}
