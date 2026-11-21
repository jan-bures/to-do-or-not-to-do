//! Where the app keeps its pairing and its cache, outside the repo and per user:
//!
//! | File | Where | Contents |
//! |---|---|---|
//! | `config.json` | `%APPDATA%\to-do-or-not-to-do\` | the server URL learnt at pairing and the device ID |
//! | `device-token.bin` | same | the device token, encrypted with DPAPI for the current user |
//! | `cache.sqlite` | `%LOCALAPPDATA%\to-do-or-not-to-do\` | the sync engine's replica and outbox |
//!
//! `TODO_DESKTOP_HOME` puts all three in one directory instead (tests, benchmarks, a second profile).
//! The token is never written in plain text and never logged. Signing out removes all three.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use todo_core::DeviceId;

use crate::platform::dpapi;

const APP_DIR: &str = "to-do-or-not-to-do";
const CONFIG_FILE: &str = "config.json";
const TOKEN_FILE: &str = "device-token.bin";
const CACHE_FILE: &str = "cache.sqlite";

/// The app's directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// Roaming settings: config and the sealed token.
    pub config_dir: PathBuf,
    /// Machine-local data: the cache.
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

    pub fn cache(&self) -> PathBuf {
        self.cache_dir.join(CACHE_FILE)
    }
}

/// What pairing taught the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The server's base URL, for example `https://todo.example.com`.
    pub server: String,
    /// This device's ID: its changes are recorded as `owner/<device>`.
    pub device: DeviceId,
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

/// Signs out: removes the token, the config and the cache (with SQLite's side files). Keeps the server
/// URL out of it too, so nothing of the owner's stays behind.
#[cfg(test)]
pub fn forget(paths: &Paths) -> anyhow::Result<()> {
    forget_pairing(paths)?;
    forget_cache(paths)
}

/// Removes the token and the config: the device is signed out from here on.
pub fn forget_pairing(paths: &Paths) -> anyhow::Result<()> {
    remove(&paths.token())?;
    remove(&paths.config())?;
    Ok(())
}

/// Removes the cache. Fails while a session still has it open.
pub fn forget_cache(paths: &Paths) -> anyhow::Result<()> {
    let cache = paths.cache();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = cache.clone().into_os_string();
        p.push(suffix);
        remove(Path::new(&p))?;
    }
    Ok(())
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

/// Normalizes a typed server address: trims it, adds `https://` when no scheme is given and drops a
/// trailing slash. Plain `http://` is only accepted for this machine (development servers), so the
/// token never crosses a network unencrypted.
pub fn normalize_server(input: &str) -> Result<String, &'static str> {
    let s = input.trim();
    if s.is_empty() {
        return Err("Enter your server's address.");
    }
    let url = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let url = match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.trim_end_matches('/')),
        None => url,
    };
    let (scheme, rest) = url.split_once("://").unwrap_or(("", ""));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let hostname = if host.starts_with('[') {
        host.split(']').next().map_or(host, |h| &h[1..])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    if hostname.is_empty() || hostname.contains(char::is_whitespace) {
        return Err("That doesn't look like a server address.");
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => Ok(url),
        "http" if matches!(hostname, "localhost" | "127.0.0.1" | "::1") => Ok(url),
        "http" => Err("Use https:// (plain http only works for this computer)."),
        _ => Err("The address must start with https://."),
    }
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

    #[test]
    fn a_pairing_round_trips_with_the_token_sealed() {
        let paths = temp();
        assert_eq!(load(&paths).expect("load"), None);
        let pairing = Pairing {
            config: Config {
                server: "https://todo.example.com".into(),
                device: DeviceId::new("dev-1").expect("id"),
            },
            token: "s3cret-device-token-0123456789".into(),
        };
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

        std::fs::write(paths.cache(), b"cache").expect("cache");
        forget(&paths).expect("forget");
        assert_eq!(load(&paths).expect("load"), None);
        assert!(!paths.cache().exists());
        assert_eq!(last_server(&paths), None);
        forget(&paths).expect("forgetting twice is fine");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
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
    }

    #[test]
    fn the_device_name_fits() {
        let n = device_name();
        assert!(n.starts_with("To-do"));
        assert!(n.chars().count() <= 64);
    }
}
