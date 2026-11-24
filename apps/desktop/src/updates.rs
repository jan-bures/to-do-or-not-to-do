//! Signed Velopack updates are verified before startup apply or before the worker offers
//! restart and quit. The UI only rechecks the verified file's metadata.

mod history;
mod rejections;
mod release;
mod state;

use rejections::Rejections;
use release::{APP_ID, CHANNEL, MAX_FEED, MAX_PROOF, Receipt, Rejection, Release};
use state::{Check, Schedule};
pub use state::{FOCUS_IDLE, Failure, Snapshot, State};

use semver::Version;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};
use todo_core::time::Timestamp;
use tokio::sync::watch;
use ureq::tls::{RootCerts, TlsConfig};
use velopack::locator::{LocationContext, auto_locate_app_manifest};
use velopack::sources::NoneSource;
use velopack::{Error, UpdateManager, VelopackAsset};

const REPOSITORY: &str = env!("TODO_DESKTOP_UPDATE_FEED");
const MAX_REDIRECTS: usize = 5;
const GITHUB_RELEASE_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug)]
struct ReleaseUrls {
    releases: String,
    base_host: String,
}

impl ReleaseUrls {
    fn new(repository: &str) -> Option<Self> {
        let url = url::Url::parse(repository.trim()).ok()?;
        let host = url.host_str()?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path().trim_matches('/').is_empty()
        {
            return None;
        }
        Some(Self {
            releases: format!("{}/releases", url.as_str().trim_end_matches('/')),
            base_host: host.to_owned(),
        })
    }

    fn history(&self) -> String {
        format!("{}.atom", self.releases)
    }

    fn manifest(&self, version: &Version) -> String {
        format!("{}/download/v{version}/releases.win.json", self.releases)
    }

    fn proof(&self, version: &Version) -> String {
        format!(
            "{}/download/v{version}/desktop-update.sig.json",
            self.releases
        )
    }

    fn package(&self, asset: &VelopackAsset) -> String {
        // The latest release can change between requests. Asset names and versions have already
        // been validated, and each package lives under its own immutable release tag.
        format!(
            "{}/download/v{}/{}",
            self.releases, asset.Version, asset.FileName
        )
    }

    fn redirect(&self, current: &url::Url, location: &str) -> Option<url::Url> {
        let next = current.join(location).ok()?;
        let host = next.host_str()?;
        (next.scheme() == "https"
            && next.port_or_known_default() == Some(443)
            && next.username().is_empty()
            && next.password().is_none()
            && next.fragment().is_none()
            && (host == self.base_host || GITHUB_RELEASE_HOSTS.contains(&host)))
        .then_some(next)
    }
}

/// Follow GitHub's HTTPS asset redirects without credentials. Neither transport nor GitHub
/// authorizes a release: verify a receipt against the compiled keys before touching packages.
struct Feed {
    urls: ReleaseUrls,
    agent: ureq::Agent,
    stopped: Arc<AtomicBool>,
}

// Only package byte mismatches can enter the cooldown cache. Authentication errors are handled
// before staging and cannot be turned into retriable mismatches by this path.
enum StageFailure {
    Download,
    Mismatch,
}

impl Feed {
    fn agent(https_only: bool) -> ureq::Agent {
        ureq::Agent::config_builder()
            .https_only(https_only)
            // Check Location ourselves before making any request to its target.
            .max_redirects(0)
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
        let mut address =
            url::Url::parse(url).map_err(|_| Error::Other("invalid update address".into()))?;
        for hops in 0..=MAX_REDIRECTS {
            self.ensure_running()?;
            let response = self
                .agent
                .get(address.as_str())
                .config()
                .timeout_recv_body(Some(body_timeout))
                .build()
                .call()?;
            match response.status().as_u16() {
                200 => return Ok(response.into_body().into_reader()),
                301 | 302 | 303 | 307 | 308 if hops < MAX_REDIRECTS => {
                    address = response
                        .headers()
                        .get(ureq::http::header::LOCATION)
                        .and_then(|header| header.to_str().ok())
                        .and_then(|location| self.urls.redirect(&address, location))
                        .ok_or_else(|| Error::Other("invalid update redirect".into()))?;
                }
                301 | 302 | 303 | 307 | 308 => break,
                _ => return Err(Error::Other("expected a complete 200 response".into())),
            }
        }
        Err(Error::Other("too many update redirects".into()))
    }
}

fn package_body_timeout(size: u64) -> Duration {
    // Allow a slow transfer without letting a trickling connection tie up the worker indefinitely.
    Duration::from_mins(5) + Duration::from_secs(size.div_ceil(32 * 1024))
}

fn is_missing(error: &Error) -> bool {
    matches!(error, Error::Network(error) if matches!(**error, velopack::NetworkError::Http(ureq::Error::StatusCode(404))))
}

impl Feed {
    fn latest(&self) -> Result<Option<(Version, String)>, Failure> {
        let xml = match self.get(&self.urls.history(), Duration::from_secs(30)) {
            Err(error) if is_missing(&error) => return Ok(None),
            Err(_) => return Err(Failure::Check),
            Ok(body) => read_text(body, history::MAX_HISTORY).map_err(|_| Failure::Check)?,
        };
        let versions = history::candidates(&xml).ok_or(Failure::Check)?;
        for version in versions {
            match self.get(&self.urls.manifest(&version), Duration::from_secs(30)) {
                Err(error) if is_missing(&error) => continue,
                Err(_) => return Err(Failure::Check),
                Ok(body) => {
                    return read_text(body, MAX_FEED).map(|manifest| Some((version, manifest)));
                }
            }
        }
        Ok(None)
    }

    fn proof(&self, version: &Version) -> Result<String, Failure> {
        match self.get(&self.urls.proof(version), Duration::from_secs(30)) {
            Err(error) if is_missing(&error) => Err(Failure::Verify),
            Err(_) => Err(Failure::Check),
            Ok(body) => read_text(body, MAX_PROOF),
        }
    }

    fn download(
        &self,
        asset: &VelopackAsset,
        file: &mut File,
        progress: &impl Fn(Option<u8>),
    ) -> Result<(), StageFailure> {
        let mut reader = self
            .get(&self.urls.package(asset), package_body_timeout(asset.Size))
            .map_err(|_| StageFailure::Download)?
            .take(asset.Size + 1);
        let mut buffer = vec![0; 64 * 1024];
        let mut bytes = 0;
        let mut hash = Sha256::new();
        let mut reported = 0;
        loop {
            self.ensure_running().map_err(|_| StageFailure::Download)?;
            let n = reader
                .read(&mut buffer)
                .map_err(|_| StageFailure::Download)?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])
                .map_err(|_| StageFailure::Download)?;
            hash.update(&buffer[..n]);
            bytes += n as u64;
            let percent = (100 * bytes / asset.Size).min(99) as u8;
            if percent > reported {
                progress(Some(percent));
                reported = percent;
            }
        }
        // An early EOF can be a transient transfer failure, even with a valid HTTP body.
        if bytes < asset.Size {
            return Err(StageFailure::Download);
        }
        if !release::matches_bytes(asset, bytes, &hash.finalize()) {
            return Err(StageFailure::Mismatch);
        }
        Ok(())
    }
}

fn read_text(body: impl Read, limit: u64) -> Result<String, Failure> {
    let bytes = release::read_bounded(body, limit).map_err(|_| Failure::Check)?;
    if bytes.len() as u64 > limit {
        return Err(Failure::Verify);
    }
    String::from_utf8(bytes).map_err(|_| Failure::Verify)
}

#[derive(Clone, Debug)]
pub struct Controller(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    status: watch::Sender<Snapshot>,
    worker: Mutex<Option<Connection>>,
    verified: Mutex<Option<Box<VerifiedPackage>>>,
    applying: AtomicBool,
}

#[derive(Debug)]
struct Connection {
    stopped: Arc<AtomicBool>,
    wake: Sender<Check>,
}

impl Controller {
    /// Run before GPUI owns a UI thread. `None` means Velopack is waiting for us to exit.
    pub fn startup(enabled: bool, restarted: bool, background: bool) -> Option<Self> {
        let Some(install) = enabled.then(Installation::locate).flatten() else {
            return Some(Self::with_state(None, State::NotInstalled));
        };
        let version = install.manager.get_current_version();
        let controller = Self::with_state(Some(version.to_string()), State::Disabled);
        if ReleaseUrls::new(REPOSITORY).is_none() {
            return Some(controller);
        }
        match startup_apply(
            &install.dir,
            &version,
            restarted,
            Receipt::verify,
            |package| {
                launch_apply(
                    &install.manager,
                    package.asset.clone(),
                    Apply::Restart,
                    background,
                )
                .is_ok()
            },
        ) {
            Startup::Applying => None,
            Startup::Continue(state) => {
                controller.0.status.send_modify(|s| s.set(state, false));
                Some(controller)
            }
            Startup::Retry(package) => {
                let state = package.ready();
                *controller
                    .0
                    .verified
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(package);
                controller.0.status.send_modify(|s| {
                    s.set(state, false);
                    s.restart_failed = true;
                    s.quit_deferred = true;
                });
                Some(controller)
            }
        }
    }

    fn with_state(version: Option<String>, state: State) -> Self {
        Self(Arc::new(Shared {
            status: watch::Sender::new(Snapshot::new(version, state)),
            worker: Mutex::default(),
            verified: Mutex::default(),
            applying: AtomicBool::new(false),
        }))
    }

    pub fn fixture(state: State) -> Self {
        let controller = Self::with_state(Some("0.1.0-build.146".into()), state.clone());
        controller
            .0
            .status
            .send_modify(|s| s.checked(Check::Manual, state, true));
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
        if self.0.status.send_if_modified(|s| s.begin(Check::Manual))
            && worker.wake.send(Check::Manual).is_err()
        {
            self.0
                .status
                .send_modify(|s| s.checked(Check::Manual, failed(Failure::Check), false));
        }
    }

    pub fn check_on_focus(&self) {
        let worker = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(worker) = worker.as_ref()
            && self.0.status.borrow().can_check
        {
            let _ = worker.wake.send(Check::Focus);
        }
    }

    /// Launch Velopack, letting the caller do normal Quit cleanup before the process exits.
    pub fn apply(&self, mode: Apply) -> bool {
        self.apply_with(mode, |version, mode| {
            let install = Installation::locate().ok_or(Failure::Apply)?;
            let mut verified = self
                .0
                .verified
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let package = verified
                .as_ref()
                .filter(|v| v.asset.Version == version)
                .ok_or(Failure::Verify)?;
            if !package.unchanged(&install.dir) {
                // Fail closed; a manual/automatic check can authenticate and stage it again.
                *verified = None;
                return Err(Failure::Verify);
            }
            Ok(launch_apply(&install.manager, package.asset.clone(), mode, false).is_ok())
        })
    }

    fn apply_with(
        &self,
        mode: Apply,
        launch: impl FnOnce(&str, Apply) -> Result<bool, Failure>,
    ) -> bool {
        if self.0.applying.swap(true, Ordering::AcqRel) {
            return false;
        }
        let snapshot = self.snapshot();
        let result = snapshot
            .ready_version()
            .filter(|_| mode != Apply::Quit || !snapshot.quit_deferred)
            .map(|version| launch(version, mode));
        if let Some(result) = result {
            if mode == Apply::Quit || result != Ok(true) {
                tracing::warn!(?mode, ?result, "staged desktop update launch result");
            }
            match result {
                Ok(true) => return true, // Keep the latch held until the process exits.
                Ok(false) => self.0.status.send_modify(|s| {
                    s.restart_failed = true;
                    s.quit_deferred |= mode == Apply::Quit;
                }),
                Err(failure) => {
                    let worker = self.0.worker.lock().unwrap_or_else(PoisonError::into_inner);
                    self.0
                        .status
                        .send_modify(|s| s.set(failed(failure), worker.is_some()));
                }
            }
        }
        self.0.applying.store(false, Ordering::Release);
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
        use std::hash::{BuildHasher, RandomState};
        let jitter =
            Duration::from_secs(RandomState::new().hash_one(0) % state::MAX_JITTER.as_secs());
        let mut schedule = Schedule::new(Instant::now(), jitter);
        if std::thread::Builder::new()
            .name("todo-update".into())
            .spawn(move || {
                if crate::platform::win::background_thread().is_err() {
                    control.disconnect(&run, failed(Failure::Check));
                    return;
                }
                let Some(install) = Installation::locate() else {
                    control.disconnect(&run, failed(Failure::Check));
                    return;
                };
                loop {
                    let check = match rx
                        .recv_timeout(schedule.due.saturating_duration_since(Instant::now()))
                    {
                        Ok(check) => check,
                        Err(mpsc::RecvTimeoutError::Timeout) => Check::Automatic,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if run.load(Ordering::Relaxed) {
                        break;
                    }
                    if check == Check::Focus && !schedule.focus_due(Instant::now()) {
                        continue;
                    }
                    if check != Check::Manual {
                        let began = control.for_worker(&run, |s| s.begin(check));
                        if !began {
                            // A manual request beat this timeout, or the package is already staged.
                            // Ready is stable: don't flash Checking or replace its package.
                            if control.0.status.borrow().ready_version().is_some() {
                                schedule.finished(check, true, Instant::now());
                            }
                            continue;
                        }
                    }
                    let result = stage_update(
                        &feed,
                        &install.dir,
                        &install.manager.get_current_version(),
                        check,
                        Receipt::verify,
                        |progress| {
                            control.for_worker(&run, |s| {
                                let state = State::Downloading { progress };
                                let changed = s.state != state;
                                s.set(state, true);
                                changed
                            });
                        },
                        Timestamp::now,
                    );
                    // Use the ordinary check schedule for verification failures; package
                    // mismatches have a separate per-offer cooldown.
                    schedule.finished(
                        check,
                        result.is_ok() || matches!(result, Err(Failure::Verify)),
                        Instant::now(),
                    );
                    control.for_worker(&run, |s| {
                        let state = match result {
                            Ok(Pending::Ready(package)) => {
                                let state = package.ready();
                                *control
                                    .0
                                    .verified
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner) = Some(package);
                                state
                            }
                            Ok(Pending::None) => State::UpToDate {
                                checked_at: Timestamp::now(),
                            },
                            Ok(Pending::Skipped(build)) => State::Skipped { build },
                            Err(kind) => {
                                tracing::debug!(?kind, "desktop update check failed");
                                failed(kind)
                            }
                        };
                        s.checked(check, state, true);
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
        self.0.status.send_if_modified(|s| {
            let state = s.state.clone();
            set_unless_ready(s, state, true)
        });
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

struct Installation {
    manager: UpdateManager,
    dir: PathBuf,
}

impl Installation {
    fn locate() -> Option<Self> {
        let locator = auto_locate_app_manifest(LocationContext::FromCurrentExe).ok()?;
        if locator.get_manifest_channel() != CHANNEL {
            return None;
        }
        let dir = locator.get_packages_dir();
        let manager = UpdateManager::new(NoneSource {}, None, None).ok()?;
        if manager.get_is_portable() || manager.get_app_id() != APP_ID {
            return None;
        }
        Some(Self { manager, dir })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    size: u64,
    modified: SystemTime,
}

impl FileStamp {
    fn read(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

#[derive(Debug)]
struct VerifiedPackage {
    asset: VelopackAsset,
    path: PathBuf,
    stamp: FileStamp,
}

impl VerifiedPackage {
    fn verify(dir: &Path, asset: &VelopackAsset) -> io::Result<Option<Self>> {
        let path = dir.join(&asset.FileName);
        let stamp = FileStamp::read(&path)?;
        if stamp.size != asset.Size
            || !release::matches_file(asset, &path)?
            || stamp != FileStamp::read(&path)?
        {
            return Ok(None);
        }
        Ok(Some(Self {
            asset: asset.clone(),
            path,
            stamp,
        }))
    }

    fn unchanged(&self, dir: &Path) -> bool {
        self.path == dir.join(&self.asset.FileName)
            && FileStamp::read(&self.path).is_ok_and(|stamp| stamp == self.stamp)
    }

    fn ready(&self) -> State {
        State::Ready {
            version: self.asset.Version.clone(),
        }
    }
}

#[derive(Debug)]
enum Pending {
    None,
    Ready(Box<VerifiedPackage>),
    Skipped(u32),
}

#[derive(Debug)]
enum Startup {
    Applying,
    Continue(State),
    Retry(Box<VerifiedPackage>),
}

fn startup_apply(
    dir: &Path,
    installed: &Version,
    restarted: bool,
    verify: impl FnOnce(&Receipt) -> Result<Release, release::Invalid>,
    launch: impl FnOnce(&VerifiedPackage) -> bool,
) -> Startup {
    let state = match verified_pending(dir, installed, verify) {
        Ok(Pending::Ready(package)) if restarted => {
            // Velopack relaunched us, but a newer package is still pending. Don't loop.
            let version = &package.asset.Version;
            tracing::warn!(%version, "desktop update did not apply; skipping this version");
            if write_version_marker(&dir.join(FAILED), version).is_err() {
                tracing::warn!("could not record the failed desktop update");
                // Even without a marker, do not auto-apply this receipt again.
                let _ = std::fs::remove_file(dir.join(RECEIPT));
            }
            skipped_build(version)
                .map_or_else(|| failed(Failure::Apply), |build| State::Skipped { build })
        }
        Ok(Pending::Ready(package)) => {
            if launch(&package) {
                return Startup::Applying;
            }
            tracing::warn!("could not start the staged desktop update at launch");
            return Startup::Retry(package);
        }
        Ok(Pending::Skipped(build)) => State::Skipped { build },
        Ok(Pending::None)
            if Rejections::load(dir).has_newer_blocked(installed, Timestamp::now()) =>
        {
            failed(Failure::Verify)
        }
        Ok(Pending::None) => State::Idle,
        Err(rejected) => {
            discard_pending(dir, installed, rejected.as_ref());
            failed(Failure::Verify)
        }
    };
    Startup::Continue(state)
}

fn discard_pending(dir: &Path, installed: &Version, rejected: Option<&Rejection>) {
    let _ = std::fs::remove_file(dir.join(RECEIPT));
    if let Some(rejected) = rejected {
        let version = &rejected.version;
        let _ = std::fs::remove_file(dir.join(format!("{APP_ID}-{version}-full.nupkg")));
        Rejections::load(dir).record_mismatch(dir, rejected, Timestamp::now());
    } else if let Ok(entries) = std::fs::read_dir(dir) {
        // A corrupt receipt cannot identify the package. Remove only canonical full packages
        // newer than this install, without trusting a file name from unauthenticated metadata.
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(|name| name.strip_prefix(APP_ID))
                .and_then(|name| name.strip_prefix('-')?.strip_suffix("-full.nupkg"))
                .and_then(release::package_version)
                .is_some_and(|(version, _)| version > *installed)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn verified_pending(
    dir: &Path,
    installed: &Version,
    verify: impl FnOnce(&Receipt) -> Result<Release, release::Invalid>,
) -> Result<Pending, Option<Rejection>> {
    let signed = read_receipt(&dir.join(RECEIPT))
        .map_err(|_| None)?
        .as_ref()
        .map(verify)
        .transpose()
        .map_err(|_| None)?;
    let Some(signed) = signed.filter(|signed| signed.newer_than(installed)) else {
        // The updater can remove a package without installing it. Keep reporting a recent
        // failed restart even if that package or its receipt is gone.
        return pending_marker(dir, installed).map_err(|_| None);
    };
    // Never let Velopack identify a package just by its name. A valid receipt must name it,
    // and actual bytes on disk must still match its signed size and SHA-256.
    if let Some(build) = blocked_build(dir, &signed.version, Check::Automatic, SystemTime::now())
        .map_err(|_| None)?
    {
        return Ok(Pending::Skipped(build));
    }
    match VerifiedPackage::verify(dir, &signed.full) {
        Ok(Some(package)) => Ok(Pending::Ready(Box::new(package))),
        Ok(None) => Err(Some(signed.rejection)),
        Err(_) => Err(None), // No byte mismatch proven; don't record a cooldown.
    }
}

fn read_receipt(path: &Path) -> io::Result<Option<Receipt>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // The saved JSON contains escaped copies of both wire documents.
    let limit = 8 * (MAX_FEED + MAX_PROOF);
    let bytes = release::read_bounded(file, limit)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}

const RECEIPT: &str = ".todo-signed-update";
const FAILED: &str = ".todo-failed-update";

fn atomic_json(path: &Path, data: &impl serde::Serialize) -> io::Result<()> {
    atomic_write(path, |file| {
        serde_json::to_writer(file, data).map_err(io::Error::other)
    })
}

fn atomic_write(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    write(temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn write_version_marker(marker: &Path, version: &str) -> io::Result<()> {
    atomic_write(marker, |file| writeln!(file, "{version}"))
}

fn skipped_build(version: &str) -> Option<u32> {
    release::package_version(version).map(|(_, build)| build)
}

fn pending_marker(dir: &Path, installed: &Version) -> io::Result<Pending> {
    Ok(failed_marker(&dir.join(FAILED), SystemTime::now())?
        .filter(|(version, _)| version > installed)
        .map_or(Pending::None, |(_, build)| Pending::Skipped(build)))
}

fn blocked_build(
    dir: &Path,
    offered: &Version,
    check: Check,
    now: SystemTime,
) -> io::Result<Option<u32>> {
    let marker = dir.join(FAILED);
    let Some((failed, build)) = failed_marker(&marker, now)? else {
        return Ok(None);
    };
    if check == Check::Manual && failed == *offered {
        std::fs::remove_file(marker)?;
        return Ok(None);
    }
    Ok((*offered <= failed).then_some(build))
}

fn failed_marker(marker: &Path, now: SystemTime) -> io::Result<Option<(Version, u32)>> {
    let file = match File::open(marker) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let expired = now
        .duration_since(file.metadata()?.modified()?)
        .is_ok_and(|age| age >= Duration::from_hours(24));
    let bytes = release::read_bounded(file, 256)?;
    let version = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| release::package_version(s.trim()));
    if expired || version.is_none() {
        std::fs::remove_file(marker)?;
        return Ok(None);
    }
    Ok(version)
}

fn stage_update(
    feed: &Feed,
    dir: &Path,
    installed: &Version,
    check: Check,
    verify: impl FnOnce(&Receipt) -> Result<Release, release::Invalid>,
    downloading: impl Fn(Option<u8>),
    now: impl Fn() -> Timestamp,
) -> Result<Pending, Failure> {
    let Some((tag, manifest)) = feed.latest()? else {
        return pending_marker(dir, installed).map_err(|_| Failure::Apply);
    };
    let receipt = Receipt {
        manifest,
        proof: feed.proof(&tag)?,
    };
    let signed = verify(&receipt).map_err(|_| Failure::Verify)?;
    if signed.version != tag {
        return Err(Failure::Verify);
    }
    if !signed.newer_than(installed) {
        return pending_marker(dir, installed).map_err(|_| Failure::Apply);
    }
    if let Some(build) =
        blocked_build(dir, &signed.version, check, SystemTime::now()).map_err(|_| Failure::Apply)?
    {
        return Ok(Pending::Skipped(build));
    }
    let mut rejections = Rejections::load(dir);
    if check != Check::Manual && rejections.blocks(&signed.rejection, now()) {
        return Err(Failure::Verify);
    }
    // A previous build may have left a full package without a usable receipt. Reuse it only
    // after authenticating the current receipt and hashing the full file against that receipt.
    if let Ok(Some(package)) = VerifiedPackage::verify(dir, &signed.full) {
        atomic_json(&dir.join(RECEIPT), &receipt).map_err(|_| Failure::Download)?;
        rejections.clear(dir, &signed.rejection);
        return Ok(Pending::Ready(Box::new(package)));
    }
    downloading(None);
    let package = match stage_verified(feed, dir, &signed, &receipt, &downloading) {
        Ok(package) => package,
        Err(StageFailure::Mismatch) => {
            rejections.record_mismatch(dir, &signed.rejection, now());
            return Err(Failure::Verify);
        }
        Err(StageFailure::Download) => return Err(Failure::Download),
    };
    rejections.clear(dir, &signed.rejection);
    tracing::info!("desktop update staged for next launch");
    Ok(Pending::Ready(Box::new(package)))
}

fn stage_verified(
    feed: &Feed,
    dir: &Path,
    release: &Release,
    receipt: &Receipt,
    progress: &impl Fn(Option<u8>),
) -> Result<VerifiedPackage, StageFailure> {
    // Native Velopack delta reconstruction can produce a different ZIP from the original full
    // package. Stage the signed full bytes directly, without letting download_updates extract
    // a replacement Update.exe or letting any unauthenticated file qualify as pending.
    std::fs::create_dir_all(dir).map_err(|_| StageFailure::Download)?;
    let mut partial = tempfile::Builder::new()
        .suffix(".partial")
        .tempfile_in(dir)
        .map_err(|_| StageFailure::Download)?;
    feed.download(&release.full, partial.as_file_mut(), progress)?;
    partial
        .as_file()
        .sync_all()
        .map_err(|_| StageFailure::Download)?;
    feed.ensure_running().map_err(|_| StageFailure::Download)?;
    let dest = dir.join(&release.full.FileName);
    partial.persist(&dest).map_err(|_| StageFailure::Download)?;
    // Cache the hash-verified on-disk file, not just the bytes streamed from the feed.
    let result = (|| {
        let package = VerifiedPackage::verify(dir, &release.full)
            .map_err(|_| StageFailure::Download)?
            .ok_or(StageFailure::Mismatch)?;
        atomic_json(&dir.join(RECEIPT), receipt).map_err(|_| StageFailure::Download)?;
        Ok(package)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
        let _ = std::fs::remove_file(dir.join(RECEIPT));
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    Restart,
    Quit,
}

fn launch_apply(
    manager: &UpdateManager,
    asset: VelopackAsset,
    mode: Apply,
    background: bool,
) -> Result<(), Error> {
    // Explicit restart is foreground; automatic startup apply preserves autostart behavior.
    manager.wait_exit_then_apply_updates(
        asset,
        true,
        mode == Apply::Restart,
        background.then(|| "--background".to_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_now_publishes_before_waking_and_never_queues_a_second_check() {
        let controller = Controller::fixture(failed(Failure::Download));
        let (wake, requests) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        *controller.0.worker.lock().expect("updater lock") = Some(Connection {
            stopped: stopped.clone(),
            wake,
        });
        let mut observer = controller.subscribe();
        controller.check_now();
        assert!(observer.has_changed().expect("watch is open"));
        assert_eq!(
            observer.borrow_and_update().state,
            State::Checking { manual: true }
        );
        assert_eq!(requests.try_recv(), Ok(Check::Manual));
        assert!(
            !controller.for_worker(&stopped, |s| s.begin(Check::Automatic)),
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
    fn quit_applies_only_a_ready_package_without_restarting() {
        let controller = Controller::fixture(State::Downloading { progress: Some(99) });
        assert!(!controller.apply_with(Apply::Quit, |_, _| panic!("not staged")));
        let controller = ready_controller();
        assert!(controller.apply_with(Apply::Quit, |version, mode| {
            assert_eq!(version, "0.1.0-build.147");
            assert_eq!(mode, Apply::Quit);
            Ok(true)
        }));
    }

    fn ready_controller() -> Controller {
        Controller::fixture(State::Ready {
            version: "0.1.0-build.147".into(),
        })
    }

    #[test]
    fn failed_quit_defers_to_startup_but_allows_explicit_restart() {
        let controller = ready_controller();
        assert!(!controller.apply_with(Apply::Quit, |_, _| Ok(false)));
        assert!(controller.snapshot().quit_deferred);
        assert!(controller.snapshot().restart_failed);
        assert!(controller.snapshot().footer_action());
        assert!(!controller.apply_with(Apply::Quit, |_, _| panic!("quit deferred")));
        assert!(controller.apply_with(Apply::Restart, |_, mode| {
            assert_eq!(mode, Apply::Restart);
            Ok(true)
        }));
    }

    #[test]
    fn apply_failure_invalidates_ready_and_publishes_the_error() {
        let controller = ready_controller();
        assert!(!controller.apply_with(Apply::Restart, |_, _| Err(Failure::Verify)));
        let state = controller.snapshot();
        assert!(matches!(
            state.state,
            State::Failed {
                kind: Failure::Verify,
                ..
            }
        ));
        assert!(!state.restart_failed);
        assert!(state.ready_version().is_none());
    }

    #[test]
    fn apply_latch_blocks_while_launching_and_after_success() {
        let controller = ready_controller();
        assert!(controller.apply_with(Apply::Restart, |_, _| {
            assert!(!controller.apply_with(Apply::Quit, |_, _| panic!("launch in flight")));
            Ok(true)
        }));
        assert!(!controller.apply_with(Apply::Quit, |_, _| panic!("updater already launched")));
    }

    #[test]
    fn failed_marker_clears_only_for_its_manual_offer_or_after_24_hours() {
        let dir = tempfile::tempdir().expect("packages directory");
        let marker = dir.path().join(FAILED);
        let older = Version::parse("0.1.0-build.9").expect("older build");
        let failed = Version::parse("0.1.0-build.10").expect("failed build");
        let newer = Version::parse("0.1.0-build.11").expect("newer build");
        let write = || {
            write_version_marker(&marker, "0.1.0-build.10").expect("write marker");
            FileStamp::read(&marker).expect("marker metadata").modified
        };
        let now = write();
        let blocked = |version, check, at| {
            blocked_build(dir.path(), version, check, at).expect("read marker")
        };
        assert_eq!(blocked(&older, Check::Manual, now), Some(10));
        assert_eq!(blocked(&newer, Check::Manual, now), None);
        assert_eq!(blocked(&failed, Check::Focus, now), Some(10));
        assert!(matches!(
            verified_pending(dir.path(), &older, |_| panic!("no receipt")),
            Ok(Pending::Skipped(10))
        ));
        assert_eq!(blocked(&failed, Check::Manual, now), None);
        assert!(!marker.exists());
        assert_eq!(blocked(&failed, Check::Automatic, now), None);

        let now = write();
        assert_eq!(
            blocked(
                &failed,
                Check::Automatic,
                now + Duration::from_hours(24) - Duration::from_secs(1)
            ),
            Some(10)
        );
        assert_eq!(
            blocked(&failed, Check::Automatic, now + Duration::from_hours(24)),
            None
        );
        assert!(!marker.exists());
    }

    #[test]
    fn large_packages_get_a_larger_body_budget_without_a_global_cap() {
        assert_eq!(
            package_body_timeout(32 * 1024 + 1),
            Duration::from_secs(302)
        );
        assert_eq!(
            package_body_timeout(256 * 1024 * 1024),
            Duration::from_secs(8492)
        );
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
    fn feed_derives_release_history_and_pins_packages_to_tags() {
        let urls =
            ReleaseUrls::new(" https://todo.example.com/owner/project/// ").expect("feed URL");
        assert_eq!(
            urls.history(),
            "https://todo.example.com/owner/project/releases.atom"
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

    fn redirect_feed(replies: Vec<(&'static str, &'static str)>) -> (Feed, Arc<Mutex<usize>>) {
        let calls = Arc::new(Mutex::new(0));
        let observed = calls.clone();
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .middleware(
                move |req: ureq::http::Request<ureq::SendBody>,
                      _next: ureq::middleware::MiddlewareNext| {
                    let mut seen = observed.lock().expect("request count");
                    let (address, location) =
                        replies.get(*seen).expect("unexpected redirect request");
                    assert_eq!(req.uri().to_string(), *address);
                    assert!(
                        !req.headers()
                            .contains_key(ureq::http::header::AUTHORIZATION)
                    );
                    *seen += 1;
                    let response = ureq::http::Response::builder();
                    if location.is_empty() {
                        Ok(response
                            .status(200)
                            .body(ureq::Body::builder().data(b"asset".to_vec()))
                            .expect("body"))
                    } else {
                        Ok(response
                            .status(302)
                            .header(ureq::http::header::LOCATION, *location)
                            .body(ureq::Body::builder().data(Vec::new()))
                            .expect("redirect"))
                    }
                },
            )
            .build()
            .into();
        let urls = ReleaseUrls::new("https://todo.example.com/owner/project").expect("fork feed");
        (
            Feed {
                urls,
                agent,
                stopped: Arc::default(),
            },
            calls,
        )
    }

    #[test]
    fn follows_at_most_five_redirects_on_the_feed_and_github_hosts() {
        let replies = vec![
            (
                "https://todo.example.com/owner/project/releases.atom",
                "/relative",
            ),
            (
                "https://todo.example.com/relative",
                "https://GitHub.com:443/asset",
            ),
            (
                "https://github.com/asset",
                "//objects.githubusercontent.com/asset",
            ),
            (
                "https://objects.githubusercontent.com/asset",
                "https://release-assets.githubusercontent.com/asset?temporary=1",
            ),
            (
                "https://release-assets.githubusercontent.com/asset?temporary=1",
                "https://todo.example.com/package",
            ),
            ("https://todo.example.com/package", ""),
        ];
        let (feed, calls) = redirect_feed(replies.clone());
        let body = feed
            .get(&feed.urls.history(), Duration::from_secs(1))
            .expect("allowed chain");
        assert_eq!(read_text(body, 8), Ok("asset".into()));
        assert_eq!(*calls.lock().expect("request count"), 6);

        let mut chain = replies;
        chain[5].1 = "/another";
        let (feed, calls) = redirect_feed(chain);
        let result = feed.get(&feed.urls.history(), Duration::from_secs(1));
        assert!(matches!(result, Err(Error::Other(msg)) if msg == "too many update redirects"));
        assert_eq!(*calls.lock().expect("request count"), 6);
    }

    #[test]
    fn rejects_unlisted_hosts_non_default_ports_and_non_https_redirects_before_requesting() {
        for bad in [
            "https://untrusted.example.com/asset",
            "https://release-assets.githubusercontent.com.example.com/asset",
            "https://raw.githubusercontent.com/asset",
            "http://release-assets.githubusercontent.com/asset",
            "https://you@github.com/asset",
            "https://github.com:8443/x",
            "https://todo.example.com:8443/x",
            "https://GITHUB.COM./x",
            "https://192.0.2.1/x",
            "https://[2001:db8::1]/x",
        ] {
            let (feed, calls) = redirect_feed(vec![
                (
                    "https://todo.example.com/owner/project/releases.atom",
                    "https://github.com/asset",
                ),
                ("https://github.com/asset", bad),
            ]);
            let result = feed.get(&feed.urls.history(), Duration::from_secs(1));
            assert!(
                matches!(result, Err(Error::Other(msg)) if msg == "invalid update redirect"),
                "{bad}"
            );
            assert_eq!(*calls.lock().expect("request count"), 2);
        }
    }

    #[test]
    fn empty_feed_disables_worker_even_for_an_install() {
        let controller = Controller::with_state(Some("0.1.0-build.9".into()), State::Disabled);
        assert!(controller.start_from("").is_none());
        assert_eq!(controller.snapshot().state, State::Disabled);
        assert!(!controller.snapshot().can_check);
    }

    fn serve_update(
        responses: Vec<(&'static str, u16, Vec<u8>)>,
    ) -> (Feed, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let feed = loopback_feed(listener.local_addr().expect("test server address"));
        let server = std::thread::spawn(move || {
            for (route, code, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("request for {route} missing: {e}"),
                    }
                };
                stream.set_nonblocking(false).expect("blocking stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("read deadline");
                let mut request = BufReader::new(&mut stream);
                let mut line = String::new();
                request.read_line(&mut line).expect("request line");
                assert_eq!(line, format!("GET {route} HTTP/1.1\r\n"));
                loop {
                    line.clear();
                    if request.read_line(&mut line).expect("header") == 0 || line == "\r\n" {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .expect("response headers");
                stream.write_all(&body).expect("response body");
            }
        });
        (feed, server)
    }

    #[test]
    fn only_verified_full_bytes_are_staged_and_pending_files_are_rehashed() {
        use ed25519_dalek::SigningKey;
        use serde_json::json;

        const FEED_ROUTE: &str = "/releases/download/v0.1.0-build.10/releases.win.json";
        const PROOF_ROUTE: &str = "/releases/download/v0.1.0-build.10/desktop-update.sig.json";
        const FULL_ROUTE: &str =
            "/releases/download/v0.1.0-build.10/todo-desktop-0.1.0-build.10-full.nupkg";
        const PACKAGE: &[u8] = b"fixture package";
        const CORRUPT: &[u8; PACKAGE.len()] = b"fixture tamper!";
        let dir = tempfile::tempdir().expect("packages directory");
        let installed = Version::parse("0.1.0-build.9").expect("installed version");
        let offered = Version::parse("0.1.0-build.10").expect("offered version");
        let name = "todo-desktop-0.1.0-build.10-full.nupkg";
        let key = SigningKey::from_bytes(&[5; 32]);
        let verify = |receipt: &Receipt| receipt.verify_with([key.verifying_key()]);
        let manifest = json!({"Assets":[{
            "PackageId":"todo-desktop", "Version":offered.to_string(), "Type":"Full", "FileName":name,
            "SHA256":hex::encode(Sha256::digest(PACKAGE)), "Size":PACKAGE.len()
        }]}).to_string();
        let tag = format!("v{offered}");
        let receipt = release::test_receipt(&key, &tag, manifest.clone());
        let proof = &receipt.proof;
        let signed = verify(&receipt).expect("valid fixture");
        let offer = || (FEED_ROUTE, 200, manifest.as_bytes().to_vec());
        let valid_proof = || (PROOF_ROUTE, 200, proof.as_bytes().to_vec());
        let start = Timestamp::now();
        let now = std::cell::Cell::new(start);
        let set_hours = |hours: i64| {
            now.set(Timestamp::from_second(start.as_second() + hours * 3600).expect("valid time"));
        };
        let check = |requests: Vec<_>, current: &Version, mode| {
            let mut history = vec![("/releases.atom", 200, history::atom(&[&tag]).into_bytes())];
            history.extend(requests);
            let (feed, server) = serve_update(history);
            let result = stage_update(
                &feed,
                dir.path(),
                current,
                mode,
                verify,
                |_| {},
                || now.get(),
            )
            .map(|pending| match pending {
                Pending::Ready(package) => Some(package.asset.Version),
                Pending::None => None,
                Pending::Skipped(build) => panic!("unexpected skipped build {build}"),
            });
            server.join().expect("test server");
            result
        };

        assert_eq!(
            check(
                vec![offer(), (PROOF_ROUTE, 404, vec![])],
                &installed,
                Check::Automatic
            ),
            Err(Failure::Verify)
        );
        assert_eq!(
            check(
                vec![offer(), (PROOF_ROUTE, 200, b"{}".to_vec())],
                &installed,
                Check::Automatic
            ),
            Err(Failure::Verify)
        );
        let wrong_tag = release::test_receipt(
            &key,
            "v0.1.0-build.11",
            manifest.replace("build.10", "build.11"),
        );
        assert_eq!(
            check(
                vec![
                    (FEED_ROUTE, 200, wrong_tag.manifest.into_bytes()),
                    (PROOF_ROUTE, 200, wrong_tag.proof.into_bytes())
                ],
                &installed,
                Check::Manual,
            ),
            Err(Failure::Verify),
            "a valid signature never overrides a tag mismatch",
        );
        assert_eq!(std::fs::read_dir(dir.path()).expect("packages").count(), 0);
        let untrusted = Receipt {
            manifest: manifest.clone(),
            proof: "{}".into(),
        };
        atomic_json(&dir.path().join(RECEIPT), &untrusted).expect("unsigned receipt fixture");
        let no_launch = |_: &VerifiedPackage| panic!("no verified package to apply");
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, no_launch),
            Startup::Continue(State::Failed {
                kind: Failure::Verify,
                ..
            })
        ));
        assert_eq!(std::fs::read_dir(dir.path()).expect("packages").count(), 0);
        let package = dir.path().join(name);
        assert_eq!(
            check(
                vec![offer(), valid_proof(), (FULL_ROUTE, 200, b"short".to_vec())],
                &installed,
                Check::Automatic,
            ),
            Err(Failure::Download),
            "a complete HTTP response shorter than the signed size is retryable",
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("packages").count(),
            0,
            "a short response leaves neither a partial package nor a cooldown",
        );
        assert_eq!(
            check(
                vec![offer(), valid_proof(), (FULL_ROUTE, 200, PACKAGE.to_vec())],
                &installed,
                Check::Automatic
            ),
            Ok(Some(offered.to_string()))
        );
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, |cached| {
                assert_eq!(cached.asset.Version, offered.to_string());
                assert!(cached.unchanged(dir.path()));
                true
            }),
            Startup::Applying
        ));
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, |_| false),
            Startup::Retry(_)
        ));
        std::fs::remove_file(dir.path().join(RECEIPT)).expect("forget receipt for legacy test");
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, no_launch),
            Startup::Continue(State::Idle)
        ));
        assert_eq!(
            check(vec![offer(), valid_proof()], &installed, Check::Automatic),
            Ok(Some(offered.to_string())),
            "authenticate and reuse existing bytes without a package request",
        );
        std::fs::remove_file(&package).expect("reset staged package fixture");
        std::fs::remove_file(dir.path().join(RECEIPT)).expect("reset receipt fixture");
        assert_eq!(
            check(vec![offer(), valid_proof()], &offered, Check::Manual),
            Ok(None)
        );
        for hour in [0, 1] {
            set_hours(hour);
            assert_eq!(
                check(
                    vec![offer(), valid_proof(), (FULL_ROUTE, 200, CORRUPT.to_vec())],
                    &installed,
                    Check::Manual
                ),
                Err(Failure::Verify)
            );
            assert_eq!(
                std::fs::read_dir(dir.path()).expect("packages").count(),
                1,
                "only the rejection marker may remain"
            );
        }

        assert!(Rejections::load(dir.path()).blocks(&signed.rejection, now.get()));
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, no_launch),
            Startup::Continue(State::Failed {
                kind: Failure::Verify,
                ..
            })
        ));
        assert_eq!(
            check(vec![offer(), valid_proof()], &installed, Check::Automatic),
            Err(Failure::Verify)
        );
        let changed = release::test_receipt(&key, &tag, format!("{manifest}\n"));
        assert_eq!(
            check(
                vec![
                    (FEED_ROUTE, 200, changed.manifest.into_bytes()),
                    (PROOF_ROUTE, 200, changed.proof.into_bytes()),
                    (FULL_ROUTE, 200, PACKAGE.to_vec())
                ],
                &installed,
                Check::Automatic
            ),
            Ok(Some(offered.to_string()))
        );
        std::fs::remove_file(dir.path().join(RECEIPT)).expect("forget receipt for legacy test");
        std::fs::write(&package, PACKAGE).expect("unsigned legacy pending package");
        assert!(matches!(
            verified_pending(dir.path(), &installed, verify),
            Ok(Pending::None)
        ));

        std::fs::remove_file(&package)
            .expect("reset legacy package fixture before cooldown retries");
        for (before, retry) in [(5, 6), (23, 24), (47, 48)] {
            set_hours(before);
            assert_eq!(
                check(vec![offer(), valid_proof()], &installed, Check::Automatic),
                Err(Failure::Verify),
                "no automatic download while cooling down",
            );
            set_hours(retry);
            let body = match retry {
                6 => vec![0; PACKAGE.len() + 1],
                24 => CORRUPT.to_vec(),
                _ => PACKAGE.to_vec(),
            };
            assert_eq!(
                check(
                    vec![offer(), valid_proof(), (FULL_ROUTE, 200, body)],
                    &installed,
                    Check::Automatic
                ),
                if retry == 48 {
                    Ok(Some(offered.to_string()))
                } else {
                    Err(Failure::Verify)
                },
            );
        }
        set_hours(47);
        assert!(
            !Rejections::load(dir.path()).blocks(&signed.rejection, now.get()),
            "successful retry clears the marker"
        );
        let Pending::Ready(cached) =
            verified_pending(dir.path(), &installed, verify).expect("receipt verified")
        else {
            panic!("verified package missing")
        };
        std::fs::write(&package, CORRUPT).expect("tamper with staged package");
        File::options()
            .write(true)
            .open(&package)
            .expect("staged file")
            .set_modified(cached.stamp.modified + Duration::from_secs(1))
            .expect("change mtime");
        assert!(!cached.unchanged(dir.path()));
        assert_eq!(
            verified_pending(dir.path(), &installed, verify)
                .expect_err("pending verification must fail"),
            Some(signed.rejection)
        );
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, no_launch),
            Startup::Continue(State::Failed {
                kind: Failure::Verify,
                ..
            })
        ));
        assert!(!package.exists());
        assert!(!dir.path().join(RECEIPT).exists());
        std::fs::write(&package, PACKAGE).expect("restore the signed bytes");
        atomic_json(&dir.path().join(RECEIPT), &receipt).expect("restore receipt");
        assert!(matches!(
            startup_apply(dir.path(), &installed, true, verify, no_launch),
            Startup::Continue(State::Skipped { build: 10 })
        ));
        assert!(matches!(
            verified_pending(dir.path(), &installed, verify),
            Ok(Pending::Skipped(10))
        ));
        assert!(matches!(
            startup_apply(dir.path(), &installed, false, verify, no_launch),
            Startup::Continue(State::Skipped { build: 10 })
        ));
        assert_eq!(
            check(vec![offer(), valid_proof()], &installed, Check::Manual),
            Ok(Some(offered.to_string()))
        );
        assert!(!dir.path().join(FAILED).exists());
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
            .get(&feed.urls.history(), Duration::from_millis(100))
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
            urls: ReleaseUrls {
                releases: format!("http://{addr}/releases"),
                base_host: addr.ip().to_string(),
            },
            agent: Feed::agent(false),
            stopped: Arc::default(),
        }
    }

    #[test]
    fn release_history_falls_back_only_for_missing_platform_manifests() {
        const HISTORY: &str = "/releases.atom";
        const TOP: &str = "/releases/download/v0.1.0-build.11/releases.win.json";
        const NEXT: &str = "/releases/download/v0.1.0-build.10/releases.win.json";
        let entries = || {
            (
                HISTORY,
                200,
                history::atom(&["v0.1.0-build.10", "v0.1.0-build.11"]).into_bytes(),
            )
        };
        for (replies, expected) in [
            (
                vec![
                    entries(),
                    (TOP, 404, vec![]),
                    (NEXT, 200, b"desktop".to_vec()),
                ],
                Ok(Some((
                    Version::parse("0.1.0-build.10").expect("version"),
                    "desktop".into(),
                ))),
            ),
            (
                vec![entries(), (TOP, 404, vec![]), (NEXT, 404, vec![])],
                Ok(None),
            ),
            (vec![entries(), (TOP, 500, vec![])], Err(Failure::Check)),
            (vec![(HISTORY, 200, b"<feed".to_vec())], Err(Failure::Check)),
            (
                vec![(HISTORY, 200, vec![b' '; history::MAX_HISTORY as usize + 1])],
                Err(Failure::Check),
            ),
        ] {
            let (feed, server) = serve_update(replies);
            assert_eq!(feed.latest(), expected);
            server.join().expect("test server");
        }
    }

    #[test]
    fn missing_history_means_no_update_but_other_errors_propagate() {
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
                assert_eq!(line, "GET /releases.atom HTTP/1.1\r\n");
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
                assert_eq!(result.expect("optional manifest"), None);
            } else {
                assert!(result.is_err());
            }
        }
    }
}
