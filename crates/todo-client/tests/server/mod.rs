//! Helpers for tests against an in-process `todo-server`: the server, native clients, and transport
//! wrappers that go offline or inject faults.
#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use todo_client::native::{HttpTransport, OsEntropy, TokioClock};
use todo_client::platform::{
    EventStream, Request, Response, SseFrame, StreamOpen, Transport, TransportError,
};
use todo_client::{Backoff, ClientConfig, Storage, SyncClient};
use todo_core::ids::EventId;
use todo_core::{Actor, DeviceId, State};
use todo_server::auth::hash_token;
use todo_server::config::OwnerToken;
use todo_server::{Config, Options, RunningServer, Server};

pub const TOKEN_A: &str = "client-test-token-alpha-0123456789";
pub const TOKEN_B: &str = "client-test-token-beta-9876543210";

pub fn alpha() -> Actor {
    Actor::owner("alpha").unwrap()
}

pub fn beta() -> Actor {
    Actor::owner("beta").unwrap()
}

pub struct TestServer {
    pub running: RunningServer,
    pub dir: TempDir,
    pub config: Config,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    pub async fn start_with(f: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.path().to_owned(),
            ..Config::default()
        };
        config.auth.owner_tokens = vec![
            OwnerToken {
                device: DeviceId::new("alpha").unwrap(),
                sha256: hash_token(TOKEN_A),
            },
            OwnerToken {
                device: DeviceId::new("beta").unwrap(),
                sha256: hash_token(TOKEN_B),
            },
        ];
        // Clients under fault injection reconnect a lot.
        config.limits.sse_connects_per_minute = 100_000;
        config.limits.sse_connect_burst = 100_000;
        config.limits.sse_heartbeat_ms = 200;
        f(&mut config);
        Self::boot(dir, config).await
    }

    /// Starts a server on the data of one that was [`halt`](Self::halt)ed (on another port).
    pub async fn boot(dir: TempDir, config: Config) -> Self {
        let running = Server::bind(&config, Options::default())
            .await
            .unwrap()
            .spawn()
            .unwrap();
        Self {
            running,
            dir,
            config,
        }
    }

    pub fn url(&self) -> String {
        self.running.url()
    }

    pub fn transport(&self, token: &str) -> HttpTransport {
        HttpTransport::new(&self.url(), token).unwrap()
    }

    pub fn state(&self) -> Arc<State> {
        self.running.engine.state()
    }

    pub fn head(&self) -> u64 {
        self.running.engine.head_seq()
    }

    /// Every command ID the server stored, with its kind (`command` or `undo`).
    pub fn stored_command_ids(&self) -> BTreeSet<(String, String)> {
        let conn = rusqlite::Connection::open_with_flags(
            self.config.database_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT command_id, kind FROM commands")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    pub async fn stop(self) {
        self.running.shutdown().await.unwrap();
    }

    /// Stops the server, keeping its data.
    pub async fn halt(self) -> (TempDir, Config) {
        self.running.shutdown().await.unwrap();
        (self.dir, self.config)
    }

    /// The ID of the event at `seq`.
    pub fn event_id_at(&self, seq: u64) -> EventId {
        let events = self.running.engine.events_since(seq - 1, 1).unwrap();
        assert_eq!(events[0].seq, Some(seq));
        events[0].id
    }

    /// The server's log epoch.
    pub fn epoch(&self) -> uuid::Uuid {
        self.running.engine.epoch()
    }
}

/// Copies a stopped server's database with its WAL sidecar files (removing stale ones at `to`).
pub fn copy_db(from: &Path, to: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let (f, t) = (sidecar(from, suffix), sidecar(to, suffix));
        if f.exists() {
            std::fs::copy(&f, &t).unwrap();
        } else if t.exists() {
            std::fs::remove_file(&t).unwrap();
        }
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

pub type Client<T> = SyncClient<T, TokioClock>;

/// Test settings: fast retries and short deadlines.
pub fn config(actor: Actor) -> ClientConfig {
    let mut cfg = ClientConfig::new(actor);
    cfg.request_timeout = Duration::from_millis(1500);
    cfg.stream_idle_timeout = Duration::from_secs(2);
    cfg.backoff = Backoff {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(200),
    };
    cfg.page_limit = 7;
    cfg
}

pub fn client_with<T: Transport>(
    transport: T,
    storage: impl Storage + 'static,
    cfg: ClientConfig,
) -> Client<T> {
    SyncClient::new(transport, TokioClock, OsEntropy, storage, cfg).unwrap()
}

pub fn client<T: Transport>(
    transport: T,
    storage: impl Storage + 'static,
    actor: Actor,
) -> Client<T> {
    client_with(transport, storage, config(actor))
}

/// Runs the client on the tokio runtime.
pub fn spawn<T: Transport>(c: &Client<T>) -> tokio::task::JoinHandle<()> {
    let c = c.clone();
    tokio::spawn(async move { c.run().await })
}

/// Stops the client and waits for its loops to end.
pub async fn stop<T: Transport>(c: &Client<T>, handle: tokio::task::JoinHandle<()>) {
    c.stop();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the client stops promptly")
        .unwrap();
}

/// Waits until `cond` holds.
pub async fn eventually(what: &str, limit: Duration, mut cond: impl FnMut() -> bool) {
    let start = tokio::time::Instant::now();
    while !cond() {
        assert!(start.elapsed() < limit, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// True once the client has nothing pending, follows the server's head and shows its state.
pub fn converged<T: Transport>(c: &Client<T>, server: &TestServer) -> bool {
    c.pending().is_empty() && c.cursor() == server.head() && c.view().data_eq(&server.state())
}

/// A transport that can be switched off, and that records the paths it was asked for.
#[derive(Clone)]
pub struct Switch<T> {
    inner: T,
    pub online: Arc<AtomicBool>,
    pub paths: Arc<Mutex<Vec<String>>>,
}

impl<T> Switch<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            online: Arc::new(AtomicBool::new(true)),
            paths: Arc::default(),
        }
    }

    pub fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::SeqCst);
    }

    fn check(&self, req: &Request) -> Result<(), TransportError> {
        self.paths.lock().unwrap().push(req.path.clone());
        if self.online.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(TransportError::Network("offline (test)".into()))
        }
    }
}

/// Ends when the switch goes off, like a dropped connection.
pub struct SwitchStream<S> {
    inner: S,
    online: Arc<AtomicBool>,
}

impl<S: EventStream> EventStream for SwitchStream<S> {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        if !self.online.load(Ordering::SeqCst) {
            return None;
        }
        let frame = self.inner.next_frame().await;
        // A frame that arrives after the switch went off is lost with the connection.
        if !self.online.load(Ordering::SeqCst) {
            return None;
        }
        frame
    }
}

impl<T: Transport> Transport for Switch<T> {
    type Stream = SwitchStream<T::Stream>;

    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        self.check(&request)?;
        self.inner.send(request).await
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<Self::Stream>, TransportError> {
        self.check(&request)?;
        Ok(match self.inner.open_stream(request).await? {
            StreamOpen::Open(s) => StreamOpen::Open(SwitchStream {
                inner: s,
                online: self.online.clone(),
            }),
            StreamOpen::Refused(r) => StreamOpen::Refused(r),
        })
    }
}

/// Fault probabilities, in percent.
#[derive(Clone, Copy, Debug)]
pub struct FaultRates {
    /// The request never reaches the server.
    pub drop_request: u32,
    /// The server processes the request, but the response is lost.
    pub drop_response: u32,
    /// The response arrives late (up to `max_delay`; possibly after the client gave up).
    pub delay: u32,
    pub max_delay: Duration,
    /// Opening a stream fails.
    pub refuse_stream: u32,
    /// An open stream is cut after a random number of frames (0 to this).
    pub stream_frames: u32,
}

/// Shared fault state: a seeded generator and a switch to turn faults off.
pub struct Faults {
    pub enabled: AtomicBool,
    rng: AtomicU64,
    rates: FaultRates,
    pub injected: AtomicU64,
}

impl Faults {
    pub fn new(seed: u64, rates: FaultRates) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(true),
            rng: AtomicU64::new(seed | 1),
            rates,
            injected: AtomicU64::new(0),
        })
    }

    fn next(&self) -> u64 {
        // xorshift64*; a lost race between threads only repeats a number.
        let mut x = self.rng.load(Ordering::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn roll(&self, percent: u32) -> bool {
        let hit = self.enabled.load(Ordering::SeqCst) && (self.next() % 100) < u64::from(percent);
        if hit {
            self.injected.fetch_add(1, Ordering::Relaxed);
        }
        hit
    }

    fn delay(&self) -> Duration {
        let max = u64::try_from(self.rates.max_delay.as_millis())
            .unwrap_or(1)
            .max(1);
        Duration::from_millis(self.next() % max)
    }

    pub fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }
}

/// Wraps a transport and injects [`Faults`].
#[derive(Clone)]
pub struct Faulty<T> {
    inner: T,
    faults: Arc<Faults>,
}

impl<T> Faulty<T> {
    pub fn new(inner: T, faults: Arc<Faults>) -> Self {
        Self { inner, faults }
    }
}

pub struct FaultyStream<S> {
    inner: S,
    faults: Arc<Faults>,
    budget: u32,
}

impl<S: EventStream> EventStream for FaultyStream<S> {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        if self.faults.enabled.load(Ordering::SeqCst) {
            if self.budget == 0 {
                self.faults.injected.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            self.budget -= 1;
        }
        self.inner.next_frame().await
    }
}

impl<T: Transport> Transport for Faulty<T> {
    type Stream = FaultyStream<T::Stream>;

    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        let f = &self.faults;
        if f.roll(f.rates.drop_request) {
            return Err(TransportError::Network("request dropped (test)".into()));
        }
        let res = self.inner.send(request).await;
        if f.roll(f.rates.drop_response) {
            return Err(TransportError::Network("response dropped (test)".into()));
        }
        if f.roll(f.rates.delay) {
            tokio::time::sleep(f.delay()).await;
        }
        res
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<Self::Stream>, TransportError> {
        let f = &self.faults;
        if f.roll(f.rates.refuse_stream) {
            return Err(TransportError::Network("stream refused (test)".into()));
        }
        Ok(match self.inner.open_stream(request).await? {
            StreamOpen::Open(s) => StreamOpen::Open(FaultyStream {
                inner: s,
                faults: f.clone(),
                budget: u32::try_from(f.next() % u64::from(f.rates.stream_frames + 1)).unwrap_or(0),
            }),
            StreamOpen::Refused(r) => StreamOpen::Refused(r),
        })
    }
}
