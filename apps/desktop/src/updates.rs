//! Unsigned Velopack updates are trusted only from the paired HTTPS server. They are staged by a
//! low-priority worker and applied before GPUI on the next primary launch.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;
use todo_client::native::{Cleartext, server_origin_with};
use velopack::bundle::Manifest;
use velopack::sources::{NoneSource, UpdateSource};
use velopack::{Error, UpdateCheck, UpdateManager, VelopackAsset, VelopackAssetFeed};

const APP_ID: &str = "todo-desktop";
const CHANNEL: &str = "win";
const CHECK_INTERVAL: Duration = Duration::from_hours(6);
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
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_global(Some(Duration::from_mins(5)))
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

    fn get(&self, file: &str) -> Result<impl Read, Error> {
        self.ensure_running()?;
        let response = self
            .agent
            .get(format!("{}/{file}", self.url))
            .header("Authorization", &self.authorization)
            .call()?;
        if response.status() != 200 {
            return Err(Error::Other("expected a complete 200 response".into()));
        }
        Ok(response.into_body().into_reader())
    }
}

fn valid_asset(asset: &VelopackAsset) -> bool {
    // All fields originate in the feed. Validate before the manager can use a name as a disk path
    // (including before it calls download_release_entry), and require Velopack's SHA-256 check.
    let kind = match asset.Type.as_str() {
        "Full" => "full",
        "Delta" => "delta",
        _ => return false,
    };
    let Ok(version) = semver::Version::parse(&asset.Version) else {
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
        self.get("releases.win.json")?
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
        let mut reader = self.get(&asset.FileName)?.take(asset.Size + 1);
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
                let Ok(manager) = UpdateManager::new(feed, None, None) else {
                    return; // Not installed (e.g. a cargo build or test).
                };
                if manager.get_is_portable() || manager.get_app_id() != APP_ID {
                    return;
                }
                // Check right away, then on a fixed delay after each check. A separate thread
                // keeps the synchronous updater off both GPUI and the sync runtime.
                loop {
                    if let Err(e) = stage_update(&manager) {
                        tracing::debug!(error = %e, "desktop update check failed");
                    }
                    if rx.recv_timeout(CHECK_INTERVAL) != Err(mpsc::RecvTimeoutError::Timeout) {
                        break;
                    }
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

fn stage_update(manager: &UpdateManager) -> Result<(), Error> {
    if manager.get_update_pending_restart().is_none()
        && let UpdateCheck::UpdateAvailable(update) = manager.check_for_updates()?
    {
        manager.download_updates(&update, None)?;
        tracing::info!("desktop update staged for next launch");
    }
    Ok(())
}

/// Called only by the normal primary launch. Returns true when Update.exe is waiting for us to
/// exit so it can apply the verified package and relaunch with the same CLI arguments.
pub fn apply_pending() -> bool {
    let Ok(manager) = UpdateManager::new(NoneSource {}, None, None) else {
        return false;
    };
    if manager.get_is_portable() || manager.get_app_id() != APP_ID {
        return false;
    }
    let Some(asset) = manager.get_update_pending_restart() else {
        return false;
    };
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
        assert!(feed.get("releases.win.json").is_err());
        let listener = server.join().expect("server thread");
        assert_eq!(
            listener.accept().expect_err("no redirect").kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
