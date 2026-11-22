//! Where the app keeps its pairing and its data, outside the repo and per user:
//!
//! | File | Where | Contents |
//! |---|---|---|
//! | `config.json` | `%APPDATA%\to-do-or-not-to-do\` | the server URL learnt at pairing, the device ID and a random pairing ID |
//! | `device-token.bin` | same | the device token, encrypted with DPAPI for the current user |
//! | `pairings\<pairing ID>\cache.sqlite` | `%LOCALAPPDATA%\to-do-or-not-to-do\` | the sync engine's replica and outbox |
//! | `pairings\<pairing ID>\ui.json` | same | local sidebar and pane-width preferences |
//! | `pairings\<pairing ID>\reminders.json` | same | reminders shown and snoozed here (IDs and times only) |
//! | `pairings\<pairing ID>\attachments\` | same | attachments downloaded to be opened |
//!
//! `TODO_DESKTOP_HOME` puts everything in one directory instead (tests, benchmarks, a second profile).
//! The token is never written in plain text and never logged.
//!
//! A pairing's data lives in a directory named after that pairing, so a new pairing never picks up an
//! old cache, and cleaning up after an old pairing can't touch the current one. Signing out removes the
//! token and the config, records the intent to remove the data as `pairings\<pairing ID>.signed-out`
//! (beside the directory, so it survives a partial removal) and removes the directory; whatever couldn't
//! be removed then (a file still open, a crash) is removed at the next start, and the record last.
//!
//! **Removal never leaves our data.** It only takes directories named exactly like a pairing ID, refuses
//! to work through a `pairings` directory (or app directory) that is a junction, symlink or other
//! reparse point, and removes a reparse point found inside as a link, never following it.

use std::collections::BTreeSet;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use todo_client::Entropy as _;
use todo_core::{DeviceId, ids::CollectionId};

use crate::model::layout::PaneWidths;
use crate::platform::dpapi;

const APP_DIR: &str = "to-do-or-not-to-do";
const CONFIG_FILE: &str = "config.json";
const TOKEN_FILE: &str = "device-token.bin";
const PAIRINGS_DIR: &str = "pairings";
pub(crate) const CACHE_FILE: &str = "cache.sqlite";
const UI_FILE: &str = "ui.json";
const REMINDERS_FILE: &str = "reminders.json";
const ATTACHMENTS_DIR: &str = "attachments";
/// `<pairing ID>.signed-out`, beside a pairing's directory once its sign-in is gone: remove it.
const SIGNED_OUT: &str = ".signed-out";

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
        DataDir {
            app: self.cache_dir.clone(),
            id: pairing.clone(),
            path: self.pairings().join(pairing.as_str()),
        }
    }
}

/// One pairing's data directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataDir {
    /// The app's machine-local directory, above `pairings`.
    app: PathBuf,
    id: PairingId,
    path: PathBuf,
}

impl DataDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cache(&self) -> PathBuf {
        self.path.join(CACHE_FILE)
    }

    /// Returns local preferences, falling back to defaults if the file is missing or unreadable.
    pub fn load_ui(&self) -> UiPreferences {
        let mut ui = self.read_ui().unwrap_or_default();
        ui.panes = ui.panes.clamped();
        ui
    }

    fn read_ui(&self) -> anyhow::Result<UiPreferences> {
        self.check()?;
        let path = self.path.join(UI_FILE);
        let meta = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            meta.is_file() && todo_fs::is_plain(&meta),
            "not a plain file"
        );
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    /// Writes into the existing pairing directory, using a flushed temporary file and a durable
    /// replacement. A failed or interrupted write never leaves a partially written `ui.json`.
    pub fn save_ui(&self, prefs: &UiPreferences) -> anyhow::Result<()> {
        self.check()?;
        let mut tmp = tempfile::NamedTempFile::new_in(self.path())?;
        serde_json::to_writer(&mut tmp, prefs)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        let tmp = tmp.into_temp_path();
        todo_fs::replace_durably(&tmp, &self.path.join(UI_FILE))?;
        Ok(())
    }

    /// What this computer has shown and snoozed (`model::reminders::Schedule`); none if the file is
    /// missing or unreadable.
    pub fn load_reminders(&self) -> crate::model::reminders::Schedule {
        let read = || -> anyhow::Result<Vec<u8>> {
            self.check()?;
            let path = self.path.join(REMINDERS_FILE);
            let meta = std::fs::symlink_metadata(&path)?;
            anyhow::ensure!(
                meta.is_file() && todo_fs::is_plain(&meta),
                "not a plain file"
            );
            Ok(std::fs::read(path)?)
        };
        read()
            .map(|bytes| crate::model::reminders::Schedule::from_json(&bytes))
            .unwrap_or_default()
    }

    /// Writes the reminders' schedule like [`save_ui`](Self::save_ui): never half written.
    pub fn save_reminders(
        &self,
        schedule: &crate::model::reminders::Schedule,
    ) -> anyhow::Result<()> {
        self.check()?;
        let mut tmp = tempfile::NamedTempFile::new_in(self.path())?;
        tmp.write_all(&schedule.to_json()?)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        let tmp = tmp.into_temp_path();
        todo_fs::replace_durably(&tmp, &self.path.join(REMINDERS_FILE))?;
        Ok(())
    }

    /// Where attachments are downloaded to be opened.
    pub fn attachments(&self) -> PathBuf {
        self.path.join(ATTACHMENTS_DIR)
    }

    fn pairings(&self) -> PathBuf {
        self.app.join(PAIRINGS_DIR)
    }

    /// The record of an unfinished sign-out, beside the directory.
    fn signed_out_record(&self) -> PathBuf {
        self.pairings()
            .join(format!("{}{SIGNED_OUT}", self.id.as_str()))
    }

    /// Checks that the directories this pairing's data hangs from are plain directories, not junctions
    /// or links that would lead removal (or writes) somewhere else. Missing ones are fine.
    ///
    /// Only the app directory and `pairings` are checked, not the directories above them. A junction
    /// above (a redirected profile or `%LOCALAPPDATA%`, say) means the app directory itself lives at the
    /// junction's target, so everything removed below it is still our own data there. Refusing those
    /// would break users with redirected profiles and protect nothing.
    pub fn check_ancestors(&self) -> std::io::Result<()> {
        check_plain_dir(&self.app)?;
        check_plain_dir(&self.pairings())
    }

    /// [`check_ancestors`](Self::check_ancestors), and the pairing's directory itself.
    pub fn check(&self) -> std::io::Result<()> {
        self.check_ancestors()?;
        check_plain_dir(&self.path)
    }
}

#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct UiPreferences {
    pub collapsed_collections: BTreeSet<CollectionId>,
    #[serde(deserialize_with = "read_pane_widths")]
    pub panes: PaneWidths,
}

fn read_pane_widths<'de, D: serde::Deserializer<'de>>(de: D) -> Result<PaneWidths, D::Error> {
    // Read the whole value first so a bad field can't leave the JSON reader mid-object.
    let value = serde_json::Value::deserialize(de)?;
    Ok(if value.is_object() {
        serde_json::from_value(value).unwrap_or_default()
    } else {
        PaneWidths::default()
    })
}

/// Fails if `dir` exists and is a link (junction, symlink) or not a directory. A missing one is fine.
pub fn check_plain_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if todo_fs::is_plain(&m) && m.is_dir() => Ok(()),
        Ok(_) => Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("{} is a link or not a directory", dir.display()),
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
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

/// Records that a pairing's data is to be removed, beside its directory (never inside it), so a removal
/// that fails or is cut short is finished at the next start. The record is on disk when this returns:
/// it is written as a flushed temporary file and moved into place durably (write-through on Windows,
/// the directory flushed elsewhere). Sign-out doesn't go ahead without it.
pub fn mark_signed_out(data: &DataDir) -> std::io::Result<()> {
    data.check_ancestors()?;
    if !data.pairings().exists() {
        return Ok(()); // No data at all.
    }
    let record = data.signed_out_record();
    let tmp = data
        .pairings()
        .join(format!("{}{SIGNED_OUT}.tmp", data.id.as_str()));
    let file = match create_new(&tmp) {
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            // Left over from an attempt that was cut short. Only a plain file is ours to remove.
            let m = std::fs::symlink_metadata(&tmp)?;
            if !todo_fs::is_plain(&m) || !m.is_file() {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!("{} is a link or not a file", tmp.display()),
                ));
            }
            std::fs::remove_file(&tmp)?;
            create_new(&tmp)?
        }
        other => other?,
    };
    // Created by this attempt from here on, so it is ours to clean up.
    let synced = file.sync_all();
    drop(file);
    let written = synced.and_then(|()| todo_fs::replace_durably(&tmp, &record));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn create_new(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Withdraws a sign-out record (the sign-in couldn't be removed after all, so the data stays).
pub fn unmark_signed_out(data: &DataDir) {
    if let Err(e) = remove(&data.signed_out_record()) {
        tracing::warn!("a sign-out record couldn't be withdrawn: {e}");
    }
}

/// Removes a pairing's data directory (the cache and the downloaded attachments), then its signed-out
/// record. Never follows a junction or link: if `pairings` or the app directory is one, nothing is
/// removed; one inside the data is removed as a link. Fails while a session still has the cache open.
pub fn remove_data(data: &DataDir) -> std::io::Result<()> {
    data.check_ancestors()?;
    match std::fs::symlink_metadata(data.path()) {
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(m) if !todo_fs::is_plain(&m) || !m.is_dir() => remove_link(data.path(), &m)?,
        Ok(_) => remove_tree(data.path())?,
    }
    remove(&data.signed_out_record())
}

/// Removes a directory tree without following any reparse point in it.
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let m = std::fs::symlink_metadata(&path)?;
        if !todo_fs::is_plain(&m) {
            remove_link(&path, &m)?;
        } else if m.is_dir() {
            remove_tree(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    std::fs::remove_dir(dir)
}

/// Removes a junction, symlink or other reparse point itself (or a stray file), never its target.
fn remove_link(path: &Path, m: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    if m.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
        // For a directory junction or symlink this deletes the link, not the directory it points to.
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Finishes sign-outs that couldn't be completed: for every `<pairing ID>.signed-out` record whose name is
/// exactly a pairing ID, removes that pairing's data (see [`remove_data`]). Never `keep` (the current
/// pairing), and nothing without a record. Returns how many were finished.
pub fn sweep_signed_out(paths: &Paths, keep: Option<&PairingId>) -> usize {
    let probe = DataDir {
        app: paths.cache_dir.clone(),
        id: PairingId(String::new()),
        path: PathBuf::new(),
    };
    if let Err(e) = probe.check_ancestors() {
        tracing::warn!("not cleaning up signed-out data: {e}");
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(paths.pairings()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(SIGNED_OUT))
            .and_then(|id| PairingId::try_from(id.to_string()).ok())
        else {
            continue;
        };
        if keep.is_some_and(|k| k.as_str().eq_ignore_ascii_case(id.as_str())) {
            continue;
        }
        match remove_data(&paths.data(&id)) {
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

    /// A fresh directory per call. The clock alone isn't enough: on Windows it ticks every 100 ns, so
    /// tests running in parallel could share a directory and trip over each other's files.
    fn temp() -> Paths {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "todo-desktop-config-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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
    fn ui_preferences_round_trip_and_are_removed_with_the_pairing() {
        use crate::model::layout::Pane;
        use crate::theme::desktop;
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = Paths::in_dir(dir.path().to_owned());
        let data = paths.data(&PairingId::random());
        let other = paths.data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("pairing directory");
        let id = CollectionId::from_uuid(todo_core::ids::uuid_v7(1, [0; 10]));
        let mut prefs = UiPreferences {
            collapsed_collections: BTreeSet::from([id]),
            panes: PaneWidths::default().with(Pane::Sidebar, desktop::SIDEBAR_MAX),
        };
        data.save_ui(&prefs).expect("save preferences");
        assert_eq!(data.load_ui(), prefs);
        assert_eq!(other.load_ui(), UiPreferences::default());

        // A collapse change doesn't lose the widths, nor does a width change lose the sidebar state.
        prefs.collapsed_collections.clear();
        data.save_ui(&prefs).expect("replace preferences");
        assert_eq!(data.load_ui(), prefs);
        assert_eq!(std::fs::read_dir(data.path()).expect("contents").count(), 1);

        remove_data(&data).expect("remove pairing data");
        assert!(!data.path().exists());
    }

    #[test]
    fn the_reminders_schedule_round_trips_per_pairing() {
        use crate::model::reminders::{Schedule, Shown};
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = Paths::in_dir(dir.path().to_owned());
        let data = paths.data(&PairingId::random());
        let other = paths.data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("pairing directory");
        assert_eq!(data.load_reminders(), Schedule::default());
        let id = todo_core::ids::ReminderId::from_uuid(todo_core::ids::uuid_v7(1, [0; 10]));
        let schedule = Schedule {
            records: Default::default(),
            shown: [Shown {
                id,
                at: 1_790_000_000_000,
            }]
            .into(),
        };
        data.save_reminders(&schedule).expect("save");
        assert_eq!(data.load_reminders(), schedule);
        assert_eq!(other.load_reminders(), Schedule::default());
        std::fs::write(data.path().join(REMINDERS_FILE), "{").expect("corrupt");
        assert_eq!(data.load_reminders(), Schedule::default());
    }

    #[test]
    fn missing_or_corrupt_ui_preferences_use_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = Paths::in_dir(dir.path().to_owned());
        let data = paths.data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("pairing directory");
        assert_eq!(data.load_ui(), UiPreferences::default());
        let path = data.path().join(UI_FILE);
        for corrupt in ["{", r#"{"collapsed_collections":["not an id"]}"#] {
            std::fs::write(&path, corrupt).expect("corrupt preferences");
            assert_eq!(data.load_ui(), UiPreferences::default());
        }
    }

    #[test]
    fn malformed_panes_do_not_discard_collapsed_collections() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data = Paths::in_dir(dir.path().to_owned()).data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("pairing directory");
        let path = data.path().join(UI_FILE);
        let id = CollectionId::from_uuid(todo_core::ids::uuid_v7(1, [0; 10]));
        for panes in [
            "null",
            "17",
            "[320,400]",
            r#"{"sidebar":"too wide","detail":400}"#,
        ] {
            std::fs::write(
                &path,
                format!(r#"{{"panes":{panes},"collapsed_collections":["{id}"]}}"#),
            )
            .expect("preferences");
            assert_eq!(
                data.load_ui(),
                UiPreferences {
                    collapsed_collections: BTreeSet::from([id]),
                    panes: PaneWidths::default(),
                }
            );
        }
    }

    #[test]
    fn old_files_get_default_widths_and_stored_widths_are_clamped() {
        use crate::model::layout::{DetailPane, Pane};
        use crate::theme::desktop;

        let dir = tempfile::tempdir().expect("temp dir");
        let data = Paths::in_dir(dir.path().to_owned()).data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("pairing directory");
        let path = data.path().join(UI_FILE);
        let id = CollectionId::from_uuid(todo_core::ids::uuid_v7(1, [0; 10]));
        std::fs::write(&path, format!(r#"{{"collapsed_collections":["{id}"]}}"#))
            .expect("old preferences");
        let old = data.load_ui();
        assert_eq!(old.collapsed_collections, BTreeSet::from([id]));
        assert_eq!(old.panes, PaneWidths::default());
        for (saved, expected) in [(1., desktop::DETAIL_MIN), (560., 560.), (10000., 10000.)] {
            std::fs::write(
                &path,
                format!(r#"{{"panes":{{"sidebar":1,"detail":{saved}}}}}"#),
            )
            .expect("widths");
            let prefs = data.load_ui().panes;
            assert_eq!(
                prefs,
                PaneWidths::default()
                    .with(Pane::Sidebar, desktop::SIDEBAR_MIN)
                    .with(Pane::Detail, expected)
            );
            let columns = prefs.at(1440., DetailPane::Docked);
            assert_eq!(
                columns.width(Pane::Detail),
                expected.min(columns.max(Pane::Detail))
            );
        }
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
        assert!(
            !old_data.signed_out_record().exists(),
            "the record goes last"
        );
        assert!(new_data.cache().exists());
        remove_data(&new_data).expect("remove");
        remove_data(&new_data).expect("removing twice is fine");
        mark_signed_out(&new_data).expect("marking missing data is fine");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    /// A directory junction at `link` pointing at `target` (no admin rights needed).
    fn junction(link: &Path, target: &Path) {
        let ok = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output()
            .expect("mklink")
            .status
            .success();
        assert!(ok, "mklink /J {}", link.display());
    }

    /// A directory outside the app's data with a file in it that must survive.
    fn outside() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("outside");
        let precious = dir.path().join("precious.txt");
        std::fs::write(&precious, b"keep me").expect("write");
        (dir, precious)
    }

    #[test]
    fn a_junction_at_a_pairing_directory_is_removed_as_a_link() {
        let paths = temp();
        let (_out, precious) = outside();
        let id = PairingId::random();
        let data = paths.data(&id);
        std::fs::create_dir_all(data.pairings()).expect("pairings");
        junction(data.path(), precious.parent().expect("dir"));
        mark_signed_out(&data).expect("mark");
        assert_eq!(sweep_signed_out(&paths, None), 1);
        assert!(!data.path().exists(), "the link is gone");
        assert_eq!(std::fs::read(&precious).expect("still there"), b"keep me");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    #[test]
    fn a_junction_inside_the_data_is_never_followed() {
        let paths = temp();
        let (_out, precious) = outside();
        let data = paths.data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("data");
        std::fs::write(data.cache(), b"cache").expect("cache");
        junction(&data.attachments(), precious.parent().expect("dir"));
        remove_data(&data).expect("remove");
        assert!(!data.path().exists());
        assert_eq!(std::fs::read(&precious).expect("still there"), b"keep me");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    #[test]
    fn a_junction_above_the_data_stops_all_removal() {
        let paths = temp();
        let (out, precious) = outside();
        // `pairings` itself leads elsewhere, to a directory holding what looks like pairing data.
        let id = PairingId::random();
        let fake = out.path().join(id.as_str());
        std::fs::create_dir_all(&fake).expect("fake");
        std::fs::write(fake.join("cache.sqlite"), b"not ours").expect("file");
        std::fs::write(out.path().join(format!("{}{SIGNED_OUT}", id.as_str())), b"").expect("rec");
        std::fs::create_dir_all(&paths.cache_dir).expect("app");
        let data = paths.data(&id);
        junction(&data.pairings(), out.path());
        assert_eq!(sweep_signed_out(&paths, None), 0);
        assert!(remove_data(&data).is_err());
        assert!(mark_signed_out(&data).is_err());
        assert!(fake.join("cache.sqlite").exists());
        assert_eq!(std::fs::read(&precious).expect("still there"), b"keep me");
        std::fs::remove_dir(data.pairings()).expect("unlink");
        let _ = std::fs::remove_dir_all(&paths.config_dir);
    }

    #[test]
    fn only_exact_pairing_ids_are_swept() {
        let paths = temp();
        let id = PairingId::random();
        let upper = id.as_str().to_ascii_uppercase();
        let pairings = paths.cache_dir.join(PAIRINGS_DIR);
        for name in [upper.as_str(), "not-a-pairing", ".."] {
            std::fs::create_dir_all(pairings.join(name)).expect("dir");
            std::fs::write(pairings.join(format!("{name}{SIGNED_OUT}")), b"").expect("rec");
        }
        std::fs::create_dir_all(pairings.join("not-a-pairing").join("x")).expect("dir");
        assert_eq!(sweep_signed_out(&paths, None), 0);
        assert!(pairings.join(&upper).exists());
        assert!(pairings.join("not-a-pairing").join("x").exists());
        // The current pairing is kept, whatever the case of the record.
        let data = paths.data(&id);
        std::fs::create_dir_all(data.path()).expect("data");
        mark_signed_out(&data).expect("mark");
        assert_eq!(sweep_signed_out(&paths, Some(&id)), 0);
        assert!(data.path().exists());
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
