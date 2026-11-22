//! Unsigned Velopack updates are trusted only from the paired HTTPS server. They are staged by a
//! low-priority worker and applied before GPUI on the next primary launch.

use semver::Version;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;
use todo_client::native::{Cleartext, server_origin_with};
use ureq::tls::{RootCerts, TlsConfig};
use velopack::bundle::Manifest;
use velopack::locator::{LocationContext, auto_locate_app_manifest};
use velopack::sources::{NoneSource, UpdateSource};
use velopack::{Error, UpdateCheck, UpdateManager, VelopackAsset, VelopackAssetFeed};

const APP_ID: &str = "todo-desktop";
const CHANNEL: &str = "win";
const INITIAL_CHECK_DELAY: Duration = Duration::from_mins(1);
const CHECK_INTERVAL: Duration = Duration::from_hours(6);
const RETRY_INTERVALS: [Duration; 3] = [
    Duration::from_mins(5),
    Duration::from_mins(30),
    CHECK_INTERVAL,
];
const MAX_FEED: u64 = 1024 * 1024;
const MAX_PACKAGE: u64 = 256 * 1024 * 1024;

fn feed_url(server: &str) -> Option<String> {
    let base = server_origin_with(server, Cleartext::Refuse).ok()?;
    Some(format!("{base}/releases"))
}

/// The built-in HTTP source accepts headers but follows redirects and resolves asset names as
/// URLs. Constrain both network requests and the filenames Velopack itself will use on disk.
struct Feed {
    url: String,
    authorization: String,
    agent: ureq::Agent,
    stopped: Arc<AtomicBool>,
}

impl Feed {
    fn new(url: String, token: &str, stopped: Arc<AtomicBool>) -> Self {
        let agent = ureq::Agent::config_builder()
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
            .into();
        Self {
            url,
            authorization: format!("Bearer {token}"),
            agent,
            stopped,
        }
    }

    fn ensure_running(&self) -> Result<(), Error> {
        if self.stopped.load(Ordering::Relaxed) {
            return Err(Error::Other("update check stopped".into()));
        }
        Ok(())
    }

    fn get(&self, file: &str, body_timeout: Duration) -> Result<impl Read, Error> {
        self.ensure_running()?;
        let response = self
            .agent
            .get(format!("{}/{file}", self.url))
            .header("Authorization", &self.authorization)
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

impl UpdateSource for Feed {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, Error> {
        if channel != CHANNEL {
            return Err(Error::Other("unexpected desktop update channel".into()));
        }
        let mut bytes = Vec::new();
        self.get("releases.win.json", Duration::from_secs(30))?
            .take(MAX_FEED + 1)
            .read_to_end(&mut bytes)?;
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

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        _progress: Option<Sender<i16>>,
    ) -> Result<(), Error> {
        if !valid_asset(asset) {
            return Err(Error::Other(
                "invalid desktop package name or metadata".into(),
            ));
        }
        let mut reader = self
            .get(&asset.FileName, package_body_timeout(asset.Size))?
            .take(asset.Size + 1);
        let mut file = File::create(local_file)?;
        let mut buffer = vec![0; 64 * 1024];
        loop {
            self.ensure_running()?;
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])?;
        }
        self.ensure_running()?;
        Ok(()) // Velopack checks the exact size and hash before staging the file.
    }
}

/// Owned by the live session. Dropping it wakes a sleeping worker and stops in-flight I/O on its
/// next chunk (or timeout). Package files are install-wide and guarded by Velopack's update lock.
pub struct Worker {
    stopped: Arc<AtomicBool>,
    _wake_on_drop: Sender<()>,
}

impl Worker {
    pub fn start(server: &str, token: &str) -> Option<Self> {
        let url = feed_url(server)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let feed = Feed::new(url, token, stopped.clone());
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("todo-update".into())
            .spawn(move || {
                if let Err(e) = crate::platform::win::background_thread() {
                    tracing::debug!(error = %e, "could not lower update thread priority");
                    return;
                }
                // Let initial sync go first. Dropping the worker interrupts this wait as well as
                // the waits between subsequent checks.
                let mut delay = INITIAL_CHECK_DELAY;
                let mut backoff = CheckBackoff::default();
                let Some((manager, marker)) = installed_manager(feed) else {
                    return; // Not installed (e.g. a cargo build or test).
                };
                while rx.recv_timeout(delay) == Err(mpsc::RecvTimeoutError::Timeout) {
                    let result = stage_update(&manager, &marker);
                    if let Err(e) = &result {
                        tracing::debug!(error = %e, "desktop update check failed");
                    }
                    delay = backoff.after_check(result.is_ok());
                }
            })
            .map_err(|e| tracing::debug!(error = %e, "could not start desktop updater"))
            .ok()?;
        Some(Self {
            stopped,
            _wake_on_drop: tx,
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct CheckBackoff {
    retries: usize,
}

impl CheckBackoff {
    fn after_check(&mut self, success: bool) -> Duration {
        if success {
            self.retries = 0;
            CHECK_INTERVAL
        } else {
            let delay = RETRY_INTERVALS[self.retries];
            self.retries = (self.retries + 1).min(RETRY_INTERVALS.len() - 1);
            delay
        }
    }
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

fn failed_version(marker: &Path) -> Result<Option<Version>, Error> {
    match std::fs::read_to_string(marker) {
        Ok(value) => Ok(Some(Version::parse(value.trim())?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn record_failed_version(marker: &Path, version: &str) -> Result<(), Error> {
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

fn stage_update(manager: &UpdateManager, marker: &Path) -> Result<(), Error> {
    let failed = failed_version(marker)?;
    let pending = manager.get_update_pending_restart();
    // Do not let a failed package block checking the feed, but do not re-stage it either.
    if !pending
        .as_ref()
        .is_some_and(|asset| is_candidate(asset, failed.as_ref()))
        && let UpdateCheck::UpdateAvailable(update) = manager.check_for_updates()?
        && is_candidate(&update.TargetFullRelease, failed.as_ref())
    {
        manager.download_updates(&update, None)?;
        tracing::info!("desktop update staged for next launch");
    }
    Ok(())
}

fn should_apply(asset: &VelopackAsset, restarted: bool, marker: &Path) -> bool {
    let failed = match failed_version(marker) {
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
        if let Err(e) = record_failed_version(marker, &asset.Version) {
            tracing::warn!(error = %e, "could not record the failed desktop update");
        }
        return false;
    }
    true
}

/// Called only by the normal primary launch. Returns true when Update.exe is waiting for us to
/// exit so it can apply the verified package and relaunch with the same CLI arguments.
pub fn apply_pending(restarted: bool) -> bool {
    let Some((manager, marker)) = installed_manager(NoneSource {}) else {
        return false;
    };
    let Some(asset) = manager.get_update_pending_restart() else {
        return false;
    };
    if !should_apply(&asset, restarted, &marker) {
        return false;
    }
    match manager.wait_exit_then_apply_updates(asset, true, true, std::env::args_os().skip(1)) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "could not start the staged desktop update");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let failed = failed_version(&marker).expect("read persisted marker");
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
    fn check_backoff_caps_and_resets_after_success() {
        let mut backoff = CheckBackoff::default();
        assert_eq!(backoff.after_check(true), Duration::from_hours(6));
        assert_eq!(backoff.after_check(false), Duration::from_mins(5));
        assert_eq!(backoff.after_check(false), Duration::from_mins(30));
        assert_eq!(backoff.after_check(false), Duration::from_hours(6));
        assert_eq!(backoff.after_check(false), Duration::from_hours(6));
        assert_eq!(backoff.after_check(true), Duration::from_hours(6));
        assert_eq!(backoff.after_check(false), Duration::from_mins(5));
    }

    #[test]
    fn large_packages_get_a_larger_body_budget_without_a_global_cap() {
        assert_eq!(
            package_body_timeout(32 * 1024 + 1),
            Duration::from_secs(302)
        );
        assert_eq!(package_body_timeout(MAX_PACKAGE), Duration::from_secs(8492));
        let feed = Feed::new(
            "https://todo.example.com/releases".into(),
            "token",
            Arc::default(),
        );
        let config = feed.agent.config();
        assert_eq!(config.timeouts().global, None);
        assert!(config.timeouts().connect.is_some());
        assert!(config.timeouts().recv_response.is_some());
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
    }

    #[test]
    fn feed_uses_the_paired_https_address() {
        assert_eq!(
            feed_url(" https://todo.example.com:8443/ ").as_deref(),
            Some("https://todo.example.com:8443/releases")
        );
        assert_eq!(
            feed_url("https://todo.example.com/prefix///").as_deref(),
            Some("https://todo.example.com/prefix/releases")
        );
        for bad in [
            "",
            "http://localhost:47320",
            "http://todo.example.com",
            "https://user@todo.example.com",
            "https://todo.example.com/?token=x",
            "https://todo.example.com/#foo",
            "file:///releases",
        ] {
            assert!(feed_url(bad).is_none(), "{bad}");
        }
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
        let feed = Feed::new(format!("http://{addr}/releases"), "token", Arc::default());
        let mut body = feed
            .get("package.nupkg", Duration::from_millis(100))
            .expect("response head");
        let result = body.read_exact(&mut [0]);
        let _ = release.send(());
        server.join().expect("server thread");
        assert_eq!(
            result.expect_err("body deadline").kind(),
            std::io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn fetch_sends_the_bearer_and_does_not_follow_redirects() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let url = format!(
            "http://{}/releases",
            listener.local_addr().expect("address")
        );
        let feed = Feed::new(url, "test-device-token", Arc::default());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connect");
            let path = format!(
                "http://{}/unexpected",
                listener.local_addr().expect("address")
            );
            let mut request = BufReader::new(&mut stream);
            let mut line = String::new();
            request.read_line(&mut line).expect("request line");
            assert_eq!(line, "GET /releases/releases.win.json HTTP/1.1\r\n");
            let mut bearer_seen = false;
            loop {
                line.clear();
                if request.read_line(&mut line).expect("header") == 0 || line == "\r\n" {
                    break;
                }
                bearer_seen |=
                    line.eq_ignore_ascii_case("Authorization: Bearer test-device-token\r\n");
            }
            assert!(bearer_seen);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write response");
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            listener
        });
        assert!(
            feed.get("releases.win.json", Duration::from_secs(1))
                .is_err()
        );
        let listener = server.join().expect("server thread");
        assert_eq!(
            listener.accept().expect_err("no redirect").kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
