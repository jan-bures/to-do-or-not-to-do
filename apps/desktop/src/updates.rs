//! Unsigned Velopack updates come from the public release host. They are staged by a
//! low-priority worker and applied on restart or before GPUI on the next primary launch.

mod state;

use state::{Check, Schedule};
pub use state::{Failure, Snapshot, State};

use semver::Version;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use todo_core::time::Timestamp;
use tokio::sync::watch;
use ureq::tls::{RootCerts, TlsConfig};
use velopack::bundle::Manifest;
use velopack::locator::{LocationContext, auto_locate_app_manifest};
use velopack::sources::{NoneSource, UpdateSource};
use velopack::{Error, UpdateCheck, UpdateManager, VelopackAsset, VelopackAssetFeed};

const APP_ID: &str = "todo-desktop";
const CHANNEL: &str = "win";
const MAX_FEED: u64 = 1024 * 1024;
const MAX_PACKAGE: u64 = 256 * 1024 * 1024;

const REPOSITORY: &str = env!("TODO_DESKTOP_UPDATE_FEED");

#[derive(Debug)]
struct ReleaseUrls(String);

impl ReleaseUrls {
    fn new(repository: &str) -> Option<Self> {
        let url = url::Url::parse(repository.trim()).ok()?;
        if url.scheme() != "https"
            || !url.has_host()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path().trim_matches('/').is_empty()
        {
            return None;
        }
        Some(Self(format!(
            "{}/releases",
            url.as_str().trim_end_matches('/')
        )))
    }

    fn manifest(&self) -> String {
        format!("{}/latest/download/releases.win.json", self.0)
    }

    fn package(&self, asset: &VelopackAsset) -> String {
        // The latest release can change between requests. Asset names and versions have already
        // been validated, and each package lives under its own immutable release tag.
        format!("{}/download/v{}/{}", self.0, asset.Version, asset.FileName)
    }
}

/// Follow GitHub's HTTPS asset redirects without credentials. Keep bounded reads and validate the
/// filenames Velopack uses on disk, rather than treating manifest data as arbitrary URLs.
struct Feed {
    urls: ReleaseUrls,
    agent: ureq::Agent,
    stopped: Arc<AtomicBool>,
}

impl Feed {
    fn agent(https_only: bool) -> ureq::Agent {
        ureq::Agent::config_builder()
            .https_only(https_only)
            .max_redirects(5)
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .timeout_resolve(Some(Duration::from_secs(10)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_send_request(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_per_read(Some(Duration::from_secs(30)))
            .build()
            .into()
    }

    fn new(urls: ReleaseUrls, stopped: Arc<AtomicBool>) -> Self {
        Self {
            urls,
            agent: Self::agent(true),
            stopped,
        }
    }

    fn ensure_running(&self) -> Result<(), Error> {
        if self.stopped.load(Ordering::Relaxed) {
            return Err(Error::Other("update check stopped".into()));
        }
        Ok(())
    }

    fn get(&self, url: &str, body_timeout: Duration) -> Result<impl Read + use<>, Error> {
        self.ensure_running()?;
        let response = self
            .agent
            .get(url)
            .config()
            .timeout_recv_body(Some(body_timeout))
            .build()
            .call()?;
        if response.status() != 200 {
            return Err(Error::Other("expected a complete 200 response".into()));
        }
        Ok(response.into_body().into_reader())
    }
}

fn package_body_timeout(size: u64) -> Duration {
    // Allow a slow transfer without letting a trickling connection tie up the worker indefinitely.
    Duration::from_mins(5) + Duration::from_secs(size.div_ceil(32 * 1024))
}

fn valid_asset(asset: &VelopackAsset) -> bool {
    // All fields originate in the feed. Validate before the manager can use a name as a disk path
    // (including before it calls download_release_entry), and require Velopack's SHA-256 check.
    let kind = match asset.Type.as_str() {
        "Full" => "full",
        "Delta" => "delta",
        _ => return false,
    };
    let Ok(version) = Version::parse(&asset.Version) else {
        return false;
    };
    let Some(height) = version.pre.as_str().strip_prefix("build.") else {
        return false;
    };
    height.parse::<u32>().is_ok()
        && version.build.is_empty()
        && asset.PackageId == APP_ID
        && asset.FileName == format!("{APP_ID}-{version}-{kind}.nupkg")
        && (1..=MAX_PACKAGE).contains(&asset.Size)
        && asset.SHA256.len() == 64
        && asset.SHA256.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Feed {
    fn latest(&self) -> Result<VelopackAssetFeed, Error> {
        let response = match self.get(&self.urls.manifest(), Duration::from_secs(30)) {
            // The repository can have no releases yet, or Latest can be Android-only.
            Err(Error::Network(error))
                if matches!(
                    *error,
                    velopack::NetworkError::Http(ureq::Error::StatusCode(404))
                ) =>
            {
                return Ok(VelopackAssetFeed { Assets: vec![] });
            }
            result => result?,
        };
        let mut bytes = Vec::new();
        response.take(MAX_FEED + 1).read_to_end(&mut bytes)?;
        self.ensure_running()?;
        if bytes.len() as u64 > MAX_FEED {
            return Err(Error::Other("desktop release feed is too large".into()));
        }
        let feed: VelopackAssetFeed = serde_json::from_slice(&bytes)?;
        if !feed.Assets.iter().all(valid_asset) {
            return Err(Error::Other("invalid desktop release feed asset".into()));
        }
        Ok(feed)
    }
}

impl UpdateSource for Feed {
    fn get_release_feed(
        &self,
        channel: &str,
        app: &Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, Error> {
        if channel != CHANNEL {
            return Err(Error::Other("unexpected desktop update channel".into()));
        }
        let mut feed = self.latest()?;
        // Publishing includes the previous full entry to identify the one supported delta base.
        // Older installations skip the inapplicable delta and download the latest full package.
        if let Some(base) = feed
            .Assets
            .iter()
            .filter(|a| a.Type == "Full")
            .filter_map(|a| Version::parse(&a.Version).ok())
            .min()
            && base == app.version
            && feed.Assets.iter().filter(|a| a.Type == "Full").count() == 2
        {
            return Ok(feed);
        }
        feed.Assets.retain(|a| a.Type == "Full");
        Ok(feed)
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress: Option<Sender<i16>>,
    ) -> Result<(), Error> {
        if !valid_asset(asset) {
            return Err(Error::Other(
                "invalid desktop package name or metadata".into(),
            ));
        }
        let mut reader = self
            .get(&self.urls.package(asset), package_body_timeout(asset.Size))?
            .take(asset.Size + 1);
        let mut file = File::create(local_file)?;
        let mut buffer = vec![0; 64 * 1024];
        let mut bytes = 0;
        let mut reported = 0;
        loop {
            self.ensure_running()?;
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])?;
            bytes += n as u64;
            // Leave completion to Velopack's verification and staging, not the transfer.
            let percent = (100 * bytes / asset.Size).min(99) as i16;
            if percent > reported
                && let Some(progress) = &progress
            {
                let _ = progress.send(percent);
                reported = percent;
            }
        }
        self.ensure_running()?;
        Ok(()) // Velopack checks the exact size and hash before staging the file.
    }
}

#[derive(Clone, Debug)]
pub struct Controller(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    status: watch::Sender<Snapshot>,
    worker: Mutex<Option<Connection>>,
    dismissal_marker: Option<PathBuf>,
}

#[derive(Debug)]
struct Connection {
    stopped: Arc<AtomicBool>,
    wake: Sender<()>,
}

impl Controller {
    pub fn new(enabled: bool) -> Self {
        if enabled && let Some((manager, marker)) = installed_manager(NoneSource {}) {
            let state = if ReleaseUrls::new(REPOSITORY).is_some() {
                pending(&manager, &marker)
                    .ok()
                    .flatten()
                    .map_or(State::Idle, |a| State::Ready { version: a.Version })
            } else {
                State::Disabled
            };
            return Self::with_state(
                Some(manager.get_current_version_as_string()),
                state,
                Some(marker.with_file_name(".todo-dismissed-update")),
            );
        }
        Self::with_state(None, State::NotInstalled, None)
    }

    fn with_state(
        version: Option<String>,
        state: State,
        dismissal_marker: Option<PathBuf>,
    ) -> Self {
        let dismissed = dismissal_marker
            .as_deref()
            .and_then(|path| read_version_marker(path).ok().flatten())
            .map(|version| version.to_string());
        Self(Arc::new(Shared {
            status: watch::Sender::new(Snapshot::new(version, state).with_dismissal(dismissed)),
            worker: Mutex::default(),
            dismissal_marker,
        }))
    }

    pub fn fixture(state: State) -> Self {
        let controller = Self::with_state(Some("0.1.0-build.146".into()), state.clone(), None);
        controller.0.status.send_modify(|s| s.set(state, true));
        controller
    }

    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.0.status.subscribe()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.0.status.borrow().clone()
    }

    pub fn check_now(&self) {
        let worker = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(worker) = worker.as_ref() else {
            return;
        };
        // Reserve Checking before waking the worker: a double click, or a timeout racing
        // this click, cannot enqueue another check.
        if self.0.status.send_if_modified(Snapshot::begin) && worker.wake.send(()).is_err() {
            self.0
                .status
                .send_modify(|s| s.set(failed(Failure::Check), false));
        }
    }

    pub fn dismiss(&self, version: &str) {
        if self.0.status.send_if_modified(|s| s.dismiss(version))
            && let Some(marker) = self.0.dismissal_marker.as_deref()
            && write_version_marker(marker, version).is_err()
        {
            tracing::debug!("could not remember dismissal of desktop update indicator");
        }
    }

    /// Launch Velopack, letting the caller do normal Quit cleanup before the process exits.
    pub fn restart(&self) -> bool {
        let Some(version) = self.snapshot().ready_version().map(str::to_owned) else {
            return false;
        };
        let ready = installed_manager(NoneSource {}).and_then(|(manager, marker)| {
            let asset = pending(&manager, &marker).ok().flatten()?;
            (asset.Version == version).then_some((manager, asset))
        });
        if let Some((manager, asset)) = ready {
            if launch_apply(&manager, asset).is_ok() {
                return true;
            }
            // Velopack didn't launch. Keep the package and offer a retry.
            self.0.status.send_modify(|s| s.restart_failed = true);
        } else {
            let worker = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
            self.0
                .status
                .send_modify(|s| s.set(failed(Failure::Apply), worker.is_some()));
        }
        tracing::warn!("could not start the staged desktop update");
        false
    }

    pub fn start(&self) -> Option<Worker> {
        self.start_from(REPOSITORY)
    }

    fn start_from(&self, repository: &str) -> Option<Worker> {
        let urls = ReleaseUrls::new(repository)?;
        let mut connection = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if self.0.status.borrow().installed_version.is_none() || connection.is_some() {
            return None;
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let feed = Feed::new(urls, stopped.clone());
        let (wake, rx) = mpsc::channel();
        let control = self.clone();
        let run = stopped.clone();
        let mut schedule = Schedule::new(Instant::now());
        if std::thread::Builder::new()
            .name("todo-update".into())
            .spawn(move || {
                if crate::platform::win::background_thread().is_err() {
                    control.disconnect(&run, failed(Failure::Check));
                    return;
                }
                let Some((manager, marker)) = installed_manager(feed) else {
                    control.disconnect(&run, failed(Failure::Check));
                    return;
                };
                loop {
                    let check = match rx
                        .recv_timeout(schedule.due.saturating_duration_since(Instant::now()))
                    {
                        Ok(()) => Check::Manual,
                        Err(mpsc::RecvTimeoutError::Timeout) => Check::Automatic,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if run.load(Ordering::Relaxed) {
                        break;
                    }
                    if check == Check::Automatic {
                        let began = control.for_worker(&run, Snapshot::begin);
                        if !began {
                            // A manual request beat this timeout, or the package is already staged.
                            // Ready is stable: don't flash Checking or replace its package.
                            if control.0.status.borrow().ready_version().is_some() {
                                schedule.finished(check, true, Instant::now());
                            }
                            continue;
                        }
                    }
                    let result = stage_update(&manager, &marker, |progress| {
                        control.for_worker(&run, |s| {
                            let state = State::Downloading { progress };
                            let changed = s.state != state;
                            s.set(state, true);
                            changed
                        });
                    });
                    schedule.finished(check, result.is_ok(), Instant::now());
                    let state = match result {
                        Ok(Some(version)) => State::Ready { version },
                        Ok(None) => State::UpToDate {
                            checked_at: Timestamp::now(),
                        },
                        Err(kind) => {
                            tracing::debug!(?kind, "desktop update check failed");
                            failed(kind)
                        }
                    };
                    control.for_worker(&run, |s| {
                        s.set(state, true);
                        true
                    });
                }
            })
            .is_err()
        {
            self.unless_ready(failed(Failure::Check), false);
            return None;
        }
        *connection = Some(Connection {
            stopped: stopped.clone(),
            wake,
        });
        self.unless_ready(State::Idle, true);
        Some(Worker {
            control: self.clone(),
            stopped,
        })
    }

    fn unless_ready(&self, state: State, has_worker: bool) {
        self.0
            .status
            .send_if_modified(|s| set_unless_ready(s, state, has_worker));
    }

    fn disconnect(&self, id: &Arc<AtomicBool>, state: State) {
        let mut worker = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if worker.as_ref().is_some_and(|w| Arc::ptr_eq(id, &w.stopped)) {
            *worker = None;
            self.unless_ready(state, false);
        }
    }

    fn for_worker(&self, id: &Arc<AtomicBool>, update: impl FnOnce(&mut Snapshot) -> bool) -> bool {
        let worker = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
        worker.as_ref().is_some_and(|w| Arc::ptr_eq(id, &w.stopped))
            && self.0.status.send_if_modified(update)
    }
}

/// Owned by the primary app process, whether or not it is paired. Dropping it wakes a sleeping
/// worker and stops in-flight I/O on its next chunk (or timeout), without publishing a late result.
pub struct Worker {
    control: Controller,
    stopped: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.control.disconnect(&self.stopped, State::Idle);
    }
}

fn failed(kind: Failure) -> State {
    State::Failed {
        kind,
        at: Timestamp::now(),
    }
}

fn set_unless_ready(snapshot: &mut Snapshot, state: State, has_worker: bool) -> bool {
    if snapshot.ready_version().is_some() {
        return false;
    }
    snapshot.set(state, has_worker);
    true
}

fn installed_manager(source: impl UpdateSource + 'static) -> Option<(UpdateManager, PathBuf)> {
    let manager = UpdateManager::new(source, None, None).ok()?;
    if manager.get_is_portable() || manager.get_app_id() != APP_ID {
        return None;
    }
    // The manager does not expose its locator; use Velopack's own lookup for the marker too.
    let locator = auto_locate_app_manifest(LocationContext::FromCurrentExe).ok()?;
    Some((
        manager,
        locator.get_packages_dir().join(".todo-failed-update"),
    ))
}

fn read_version_marker(marker: &Path) -> Result<Option<Version>, Error> {
    match std::fs::read_to_string(marker) {
        Ok(value) => Ok(Some(Version::parse(value.trim())?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_version_marker(marker: &Path, version: &str) -> Result<(), Error> {
    let dir = marker
        .parent()
        .ok_or_else(|| Error::Other("missing packages directory".into()))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    writeln!(temp, "{version}")?;
    temp.as_file().sync_all()?;
    temp.persist(marker).map_err(|e| e.error)?;
    Ok(())
}

fn is_candidate(asset: &VelopackAsset, failed: Option<&Version>) -> bool {
    failed
        .is_none_or(|failed| Version::parse(&asset.Version).is_ok_and(|version| version > *failed))
}

fn pending(manager: &UpdateManager, marker: &Path) -> Result<Option<VelopackAsset>, Error> {
    let failed = read_version_marker(marker)?;
    Ok(manager
        .get_update_pending_restart()
        .filter(|a| is_candidate(a, failed.as_ref())))
}

fn stage_update(
    manager: &UpdateManager,
    marker: &Path,
    downloading: impl Fn(Option<u8>) + Sync,
) -> Result<Option<String>, Failure> {
    let failed = read_version_marker(marker).map_err(|_| Failure::Apply)?;
    if let Some(asset) = manager
        .get_update_pending_restart()
        .filter(|a| is_candidate(a, failed.as_ref()))
    {
        return Ok(Some(asset.Version));
    }
    let check = manager.check_for_updates().map_err(|_| Failure::Check)?;
    // Do not let a failed package block checking the feed, but do not re-stage it either.
    let UpdateCheck::UpdateAvailable(update) = check else {
        return Ok(None);
    };
    if !is_candidate(&update.TargetFullRelease, failed.as_ref()) {
        return Err(Failure::Apply);
    }
    downloading(None);
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel::<i16>();
        let downloading = &downloading;
        // Progress is best effort; if the relay can't start, the download still proceeds.
        let _ = std::thread::Builder::new()
            .name("todo-update-progress".into())
            .spawn_scoped(scope, move || {
                for percent in rx {
                    if (0..=100).contains(&percent) {
                        downloading(Some(percent as u8));
                    }
                }
            });
        manager.download_updates(&update, Some(tx))
    })
    .map_err(|_| Failure::Download)?;
    let asset = manager
        .get_update_pending_restart()
        .filter(|a| is_candidate(a, failed.as_ref()))
        .ok_or(Failure::Download)?;
    tracing::info!("desktop update staged for next launch");
    Ok(Some(asset.Version))
}

fn should_apply(asset: &VelopackAsset, restarted: bool, marker: &Path) -> bool {
    let failed = match read_version_marker(marker) {
        Ok(failed) => failed,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the desktop update failure marker");
            return false;
        }
    };
    if !is_candidate(asset, failed.as_ref()) {
        return false;
    }
    if restarted {
        tracing::warn!(version = %asset.Version, "desktop update did not apply; skipping this version");
        if let Err(e) = write_version_marker(marker, &asset.Version) {
            tracing::warn!(error = %e, "could not record the failed desktop update");
        }
        return false;
    }
    true
}

/// Called only by the normal primary launch. Returns true when Update.exe is waiting for us to
/// exit so it can apply the verified package and relaunch with the same CLI arguments.
pub fn apply_pending(restarted: bool) -> bool {
    if ReleaseUrls::new(REPOSITORY).is_none() {
        return false;
    }
    let Some((manager, marker)) = installed_manager(NoneSource {}) else {
        return false;
    };
    let Some(asset) = manager.get_update_pending_restart() else {
        return false;
    };
    if !should_apply(&asset, restarted, &marker) {
        return false;
    }
    match launch_apply(&manager, asset) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "could not start the staged desktop update");
            false
        }
    }
}

fn launch_apply(manager: &UpdateManager, asset: VelopackAsset) -> Result<(), Error> {
    manager.wait_exit_then_apply_updates(asset, true, true, std::env::args_os().skip(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_now_publishes_before_waking_and_never_queues_a_second_check() {
        let controller = Controller::fixture(State::Idle);
        let (wake, requests) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        *controller.0.worker.lock().expect("updater lock") = Some(Connection {
            stopped: stopped.clone(),
            wake,
        });
        let mut observer = controller.subscribe();
        controller.check_now();
        assert!(observer.has_changed().expect("watch is open"));
        assert_eq!(observer.borrow_and_update().state, State::Checking);
        assert_eq!(requests.try_recv(), Ok(()));
        assert!(
            !controller.for_worker(&stopped, Snapshot::begin),
            "a due auto-check cannot race the click"
        );
        controller.check_now();
        assert_eq!(requests.try_recv(), Err(mpsc::TryRecvError::Empty));

        let worker = Worker {
            control: controller.clone(),
            stopped: stopped.clone(),
        };
        drop(worker);
        assert_eq!(requests.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        assert!(
            !controller.for_worker(&stopped, |s| {
                s.set(
                    State::Ready {
                        version: "0.1.0-build.147".into(),
                    },
                    false,
                );
                true
            }),
            "a cancelled worker cannot publish a late result"
        );
        assert_eq!(controller.snapshot().state, State::Idle);
    }

    #[test]
    fn dismissal_survives_reopening_but_does_not_hide_a_new_version() {
        let dir = tempfile::tempdir().expect("packages directory");
        let marker = dir.path().join(".todo-dismissed-update");
        let ready = |version: &str| {
            Controller::with_state(
                Some("0.1.0-build.9".into()),
                State::Ready {
                    version: version.into(),
                },
                Some(marker.clone()),
            )
        };
        let controller = ready("0.1.0-build.10");
        controller.dismiss("0.1.0-build.10");
        let reopened = ready("0.1.0-build.10").snapshot();
        assert_eq!(reopened.ready_version(), Some("0.1.0-build.10"));
        assert_eq!(reopened.indicator_version(), None);
        assert_eq!(
            ready("0.1.0-build.11").snapshot().indicator_version(),
            Some("0.1.0-build.11")
        );
    }

    #[test]
    fn a_failed_restart_skips_the_same_version_on_later_launches() {
        let dir = tempfile::tempdir().expect("packages directory");
        let marker = dir.path().join(".todo-failed-update");
        let mut asset = VelopackAsset {
            Version: "0.1.0-build.9".into(),
            ..Default::default()
        };
        assert!(should_apply(&asset, false, &marker));
        assert!(!marker.exists());
        assert!(!should_apply(&asset, true, &marker));
        assert!(!should_apply(&asset, false, &marker));
        let failed = read_version_marker(&marker).expect("read persisted marker");
        assert!(!is_candidate(&asset, failed.as_ref()));
        asset.Version = "0.1.0-build.8".into();
        assert!(!is_candidate(&asset, failed.as_ref()));
        asset.Version = "0.1.0-build.10".into();
        assert!(is_candidate(&asset, failed.as_ref()));
        assert!(should_apply(&asset, false, &marker));
        assert!(!should_apply(&asset, true, &marker));
        assert!(!should_apply(&asset, false, &marker));
    }

    #[test]
    fn large_packages_get_a_larger_body_budget_without_a_global_cap() {
        assert_eq!(
            package_body_timeout(32 * 1024 + 1),
            Duration::from_secs(302)
        );
        assert_eq!(package_body_timeout(MAX_PACKAGE), Duration::from_secs(8492));
        let urls = ReleaseUrls::new("https://todo.example.com/owner/project").expect("feed URL");
        let feed = Feed::new(urls, Arc::default());
        let config = feed.agent.config();
        assert_eq!(config.timeouts().global, None);
        assert!(config.https_only());
        assert!(config.timeouts().connect.is_some());
        assert!(config.timeouts().recv_response.is_some());
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
    }

    #[test]
    fn feed_derives_latest_manifest_and_pins_packages_to_tags() {
        let urls =
            ReleaseUrls::new(" https://todo.example.com/owner/project/// ").expect("feed URL");
        assert_eq!(
            urls.manifest(),
            "https://todo.example.com/owner/project/releases/latest/download/releases.win.json"
        );
        let asset = VelopackAsset {
            Version: "0.1.0-build.9".into(),
            FileName: "todo-desktop-0.1.0-build.9-full.nupkg".into(),
            ..Default::default()
        };
        assert_eq!(
            urls.package(&asset),
            "https://todo.example.com/owner/project/releases/download/v0.1.0-build.9/todo-desktop-0.1.0-build.9-full.nupkg"
        );
        for bad in [
            "",
            "http://localhost:47320",
            "http://todo.example.com",
            "https://user@todo.example.com/owner/project",
            "https://todo.example.com/owner/project?token=x",
            "https://todo.example.com/owner/project#foo",
            "file:///releases",
        ] {
            assert!(ReleaseUrls::new(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn empty_feed_disables_worker_even_for_an_install() {
        let controller =
            Controller::with_state(Some("0.1.0-build.9".into()), State::Disabled, None);
        assert!(controller.start_from("").is_none());
        assert_eq!(controller.snapshot().state, State::Disabled);
        assert!(!controller.snapshot().can_check);
    }

    #[test]
    fn asset_names_are_bound_to_the_app_version_and_kind() {
        let mut asset = VelopackAsset {
            PackageId: APP_ID.into(),
            Version: "0.1.0-build.146".into(),
            Type: "Delta".into(),
            FileName: "todo-desktop-0.1.0-build.146-delta.nupkg".into(),
            SHA256: "a".repeat(64),
            Size: 100,
            ..Default::default()
        };
        assert!(valid_asset(&asset));
        let name = asset.FileName.clone();
        for bad in [
            "https://todo.example.com/package.nupkg",
            "../package.nupkg",
            r"..\package.nupkg",
            "todo-desktop-0.1.0-build.146-full.nupkg",
            "todo-desktop-0.1.0-build.900-delta.nupkg",
        ] {
            asset.FileName = bad.into();
            assert!(!valid_asset(&asset));
        }
        asset.FileName = name;
        asset.SHA256.clear();
        assert!(!valid_asset(&asset));
    }

    #[test]
    fn a_stalled_body_hits_the_per_request_deadline() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let addr = listener.local_addr().expect("address");
        let (release, wait) = mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connect");
            let mut request = BufReader::new(&mut stream);
            let mut line = String::new();
            while request.read_line(&mut line).expect("header") != 0 && line != "\r\n" {
                line.clear();
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n")
                .expect("headers");
            // The test may release us as soon as the client times out, but must finish quickly
            // even if the deadline regresses and the client waits for the whole body.
            let _ = wait.recv_timeout(Duration::from_secs(2));
            let _ = stream.write_all(b"x");
        });
        let feed = loopback_feed(addr);
        let mut body = feed
            .get(&feed.urls.manifest(), Duration::from_millis(100))
            .expect("response head");
        let result = body.read_exact(&mut [0]);
        let _ = release.send(());
        server.join().expect("server thread");
        let error = result.expect_err("body deadline");
        assert!(
            matches!(
                error
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<ureq::Error>()),
                Some(ureq::Error::Timeout(_))
            ),
            "expected a ureq timeout, got {error:?}"
        );
    }

    fn loopback_feed(addr: std::net::SocketAddr) -> Feed {
        Feed {
            urls: ReleaseUrls(format!("http://{addr}/releases")),
            agent: Feed::agent(false),
            stopped: Arc::default(),
        }
    }

    #[test]
    fn missing_latest_manifest_means_no_update_but_other_errors_propagate() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        for status in [404, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
            let feed = loopback_feed(listener.local_addr().expect("address"));
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("connect");
                let mut request = BufReader::new(&mut stream);
                let mut line = String::new();
                request.read_line(&mut line).expect("request line");
                assert_eq!(
                    line,
                    "GET /releases/latest/download/releases.win.json HTTP/1.1\r\n"
                );
                loop {
                    line.clear();
                    if request.read_line(&mut line).expect("header") == 0 || line == "\r\n" {
                        break;
                    }
                    assert!(!line.to_ascii_lowercase().starts_with("authorization:"));
                }
                write!(
                    stream,
                    "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .expect("write response");
            });
            let result = feed.latest();
            server.join().expect("server thread");
            if status == 404 {
                assert!(result.expect("optional manifest").Assets.is_empty());
            } else {
                assert!(result.is_err());
            }
        }
    }
}
