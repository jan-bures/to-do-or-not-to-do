//! The garbage collector, the budget warning and operation counting, against a real engine.

use super::*;
use crate::engine::{Clock, Engine};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use todo_core::command::NewTask;
use todo_core::ids::{AttachmentId, ListId, TaskId};

const DAY_MS: i64 = 86_400_000;
const START_MS: i64 = 1_795_000_000_000; // November 2026

/// A local store that records the operations it serves.
#[derive(Debug)]
struct Counting {
    inner: store::LocalStore,
    ops: Mutex<Vec<&'static str>>,
}

impl Counting {
    fn ops(&self) -> Vec<&'static str> {
        self.ops.lock().unwrap().clone()
    }
}

impl BlobStore for Counting {
    fn put(&self, key: &str, body: &File, len: u64) -> Result<(), StoreError> {
        self.ops.lock().unwrap().push("PUT");
        self.inner.put(key, body, len)
    }
    fn get(&self, key: &str) -> Result<Box<dyn Read + Send>, StoreError> {
        self.ops.lock().unwrap().push("GET");
        self.inner.get(key)
    }
    fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.ops.lock().unwrap().push("DELETE");
        self.inner.delete(key)
    }
    fn kind(&self) -> &'static str {
        "counting"
    }
}

struct Env {
    _dir: tempfile::TempDir,
    now: Arc<AtomicI64>,
    engine: Arc<Engine>,
    store: Arc<Counting>,
    att: Attachments,
    owner: Actor,
    inbox: ListId,
    task: TaskId,
}

fn cid() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

impl Env {
    fn new(cfg: AttachmentsConfig) -> Self {
        Self::build(cfg, true)
    }

    fn build(cfg: AttachmentsConfig, with_inbox: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicI64::new(START_MS));
        let n = now.clone();
        let clock: Clock =
            Arc::new(move || Timestamp::from_millisecond(n.load(Ordering::SeqCst)).unwrap());
        let engine = Arc::new(Engine::open(&dir.path().join("todo.sqlite3"), clock).unwrap());
        let store = Arc::new(Counting {
            inner: store::LocalStore::open(&dir.path().join("blobs")).unwrap(),
            ops: Mutex::default(),
        });
        let att = Attachments::with_store(&cfg, dir.path(), engine.clone(), store.clone()).unwrap();
        let owner = Actor::owner("pc").unwrap();
        let inbox = ListId::from_uuid(cid());
        let name = if with_inbox { "Inbox" } else { "Work" };
        engine
            .execute(
                &owner,
                cid(),
                &Command::CreateList {
                    id: inbox,
                    name: name.into(),
                    collection: None,
                    color: None,
                    placement: Default::default(),
                },
            )
            .unwrap();
        let task = TaskId::from_uuid(cid());
        engine
            .execute(
                &owner,
                cid(),
                &Command::CreateTask(Box::new(NewTask::new(task, inbox, "t"))),
            )
            .unwrap();
        Self {
            _dir: dir,
            now,
            engine,
            store,
            att,
            owner,
            inbox,
            task,
        }
    }

    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    /// Seals and stores `data`, without recording a reference.
    fn store_only(&self, id: AttachmentId, data: &[u8]) -> Result<String, AttachmentError> {
        let key = store::new_object_key()?;
        let mut spool = self.att.spool(&key, id)?;
        spool.push(data)?;
        let sealed = spool.finish()?;
        self.att.store(&key, id, &key, &sealed)?;
        Ok(key)
    }

    /// Uploads `data` as a new attachment of the task; returns its ID and the event seq.
    fn upload(&self, data: &[u8]) -> (AttachmentId, u64) {
        let id = AttachmentId::from_uuid(cid());
        let key = self.store_only(id, data).unwrap();
        let o = self
            .engine
            .execute(
                &self.owner,
                cid(),
                &Command::AddAttachment {
                    id,
                    task: self.task,
                    file_name: "f.bin".into(),
                    media_type: "application/octet-stream".into(),
                    size_bytes: data.len() as u64,
                    storage_key: key,
                },
            )
            .unwrap();
        (id, o.result.events[0].seq)
    }

    fn delete(&self, id: EntityId) -> u64 {
        let o = self
            .engine
            .execute(&self.owner, cid(), &Command::Delete { id })
            .unwrap();
        o.result.events[0].seq
    }

    fn undo(&self, seq: u64) -> u64 {
        let o = self
            .engine
            .undo(&self.owner, seq, false, Some(cid()))
            .unwrap();
        o.result.events[0].seq
    }

    fn content(&self, id: AttachmentId) -> Option<Vec<u8>> {
        let state = self.engine.state();
        let a = state.attachments.get(&id)?;
        let row = self.att.find(id, &a.storage_key).unwrap()?;
        let mut out = Vec::new();
        self.att
            .open_object(&row)
            .unwrap()
            .copy_to(&mut out)
            .unwrap();
        Some(out)
    }

    fn objects(&self) -> usize {
        self.att.registry().all().unwrap().len()
    }

    fn gc(&self) -> GcReport {
        self.att.collect_garbage().unwrap()
    }

    fn warnings(&self) -> Vec<todo_core::model::Task> {
        self.engine
            .state()
            .tasks
            .values()
            .filter(|t| t.title.starts_with("Attachment storage budget reached"))
            .cloned()
            .collect()
    }
}

fn cfg() -> AttachmentsConfig {
    AttachmentsConfig::default()
}

#[test]
fn collection_waits_out_the_retention_window_and_respects_undo() {
    let env = Env::new(cfg());
    let (a, _) = env.upload(b"keep me");
    let (b, _) = env.upload(b"delete me");
    assert_eq!(
        env.gc(),
        GcReport {
            kept: 2,
            ..GcReport::default()
        }
    );

    // Deleting only tombstones the reference; the object waits.
    let del = env.delete(EntityId::Attachment(b));
    assert_eq!(env.gc().waiting, 1);
    env.advance(29 * DAY_MS);
    let r = env.gc();
    assert_eq!((r.waiting, r.deleted), (1, 0));

    // Undo inside the window: the content is back, and the clock resets.
    env.undo(del);
    assert_eq!(env.content(b).unwrap(), b"delete me");
    assert_eq!(env.gc().kept, 2);
    env.delete(EntityId::Attachment(b));
    env.gc();
    env.advance(29 * DAY_MS);
    assert_eq!(
        env.gc().deleted,
        0,
        "the window restarted at the second deletion"
    );
    env.advance(2 * DAY_MS);
    let r = env.gc();
    assert_eq!((r.deleted, r.kept), (1, 1));
    assert_eq!(env.objects(), 1);
    assert_eq!(env.content(a).unwrap(), b"keep me");
    // The attachment can still be restored, but its content is gone.
    env.engine
        .execute(
            &env.owner,
            cid(),
            &Command::Restore {
                id: EntityId::Attachment(b),
            },
        )
        .unwrap();
    assert!(env.content(b).is_none());
}

#[test]
fn undone_creations_and_deleted_tasks_count_as_unreferenced() {
    let env = Env::new(cfg());
    let (a, created) = env.upload(b"x");
    // Undoing the upload withdraws the reference entirely; redoing it brings it back.
    let undone = env.undo(created);
    assert!(!env.engine.state().attachments.contains_key(&a));
    assert_eq!(env.gc().waiting, 1);
    env.advance(10 * DAY_MS);
    env.undo(undone);
    assert_eq!(env.gc().kept, 1);
    assert_eq!(env.content(a).unwrap(), b"x");

    // Deleting the task (not the attachment) also starts the clock.
    env.delete(EntityId::Task(env.task));
    assert_eq!(env.gc().waiting, 1);
    env.advance(31 * DAY_MS);
    assert_eq!(env.gc().deleted, 1);
    assert_eq!(
        env.store.ops().iter().filter(|o| **o == "DELETE").count(),
        1
    );
}

#[test]
fn orphans_and_abandoned_uploads_are_collected() {
    let env = Env::new(cfg());
    // Stored, but the command never ran (it failed, or timed out before committing).
    env.store_only(AttachmentId::from_uuid(cid()), b"orphan")
        .unwrap();
    assert_eq!(env.gc().waiting, 1);
    env.advance(31 * DAY_MS);
    assert_eq!(env.gc().deleted, 1);
    assert_eq!(env.objects(), 0);
}

#[test]
fn runs_are_bounded_and_at_most_hourly() {
    let env = Env::new(AttachmentsConfig {
        gc_max_deletes_per_run: 1,
        ..cfg()
    });
    for _ in 0..3 {
        env.store_only(AttachmentId::from_uuid(cid()), b"o")
            .unwrap();
    }
    assert!(env.att.collect_garbage_if_due().unwrap().is_some());
    assert!(
        env.att.collect_garbage_if_due().unwrap().is_none(),
        "ran a moment ago"
    );
    env.advance(31 * DAY_MS);
    let r = env.att.collect_garbage_if_due().unwrap().unwrap();
    assert_eq!((r.deleted, r.stopped_early), (1, true));
    env.advance(59 * 60_000);
    assert!(env.att.collect_garbage_if_due().unwrap().is_none());
    env.advance(60_000);
    assert_eq!(
        env.att.collect_garbage_if_due().unwrap().unwrap().deleted,
        1
    );
    assert_eq!(env.objects(), 1);
}

#[test]
fn every_operation_is_counted_before_it_is_sent() {
    let env = Env::new(cfg());
    let (a, _) = env.upload(b"counted");
    env.content(a).unwrap();
    env.content(a).unwrap();
    env.delete(EntityId::Attachment(a));
    env.gc();
    env.advance(31 * DAY_MS);
    env.gc();
    assert_eq!(env.store.ops(), ["PUT", "GET", "GET", "DELETE"]);
    // The month changed while waiting: the PUT and GETs are November's, the DELETE December's.
    let nov = env.att.registry().usage("2026-11").unwrap();
    assert_eq!((nov.class_a, nov.class_b), (1, 2));
    let now = env.att.registry().usage(&env.att.month()).unwrap();
    assert_eq!((now.class_a, now.class_b, now.storage_bytes), (1, 0, 0));
}

#[test]
fn refusals_file_one_warning_task_a_month() {
    let env = Env::new(AttachmentsConfig {
        budget: BudgetCaps {
            class_a_monthly: 1,
            ..BudgetCaps::default()
        },
        ..cfg()
    });
    env.upload(b"one");
    let refused = env
        .store_only(AttachmentId::from_uuid(cid()), b"two")
        .unwrap_err();
    let AttachmentError::Refused(r) = refused else {
        panic!("not refused: {refused}");
    };
    assert!(matches!(
        r,
        Refusal::Operations {
            class: OpClass::A,
            ..
        }
    ));
    assert_eq!(env.store.ops(), ["PUT"], "a refused upload sends nothing");
    env.att.warn_refused(&r);
    env.att.warn_refused(&r);
    let w = env.warnings();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].list, env.inbox);
    assert_eq!(w[0].created.by, Actor::System);
    assert!(w[0].notes.contains("Class A"), "{}", w[0].notes);

    // Next month: a fresh allowance, and a fresh warning when it runs out again.
    env.advance(31 * DAY_MS);
    env.upload(b"december");
    let Err(AttachmentError::Refused(r)) = env.store_only(AttachmentId::from_uuid(cid()), b"x")
    else {
        panic!("expected a refusal");
    };
    env.att.warn_refused(&r);
    assert_eq!(env.warnings().len(), 2);
}

#[test]
fn storage_cap_counts_sealed_bytes_and_frees_on_collection() {
    let env = Env::new(AttachmentsConfig {
        budget: BudgetCaps {
            max_storage_bytes: crypto::sealed_len(1000),
            ..BudgetCaps::default()
        },
        ..cfg()
    });
    let (a, _) = env.upload(&[1; 1000]);
    assert!(matches!(
        env.att.precheck_upload(1),
        Err(AttachmentError::Refused(Refusal::Storage { .. }))
    ));
    env.delete(EntityId::Attachment(a));
    env.gc();
    env.advance(31 * DAY_MS);
    env.gc();
    env.att.precheck_upload(1000).unwrap();
}

#[test]
fn a_warning_without_an_inbox_is_retried_later() {
    let env = Env::build(cfg(), false);
    let r = Refusal::Storage {
        used: 1,
        wanted: 1,
        cap: 1,
    };
    env.att.warn_refused(&r);
    assert!(env.warnings().is_empty());
    let inbox = ListId::from_uuid(cid());
    env.engine
        .execute(
            &env.owner,
            cid(),
            &Command::CreateList {
                id: inbox,
                name: "inbox".into(),
                collection: None,
                color: None,
                placement: Default::default(),
            },
        )
        .unwrap();
    env.att.warn_refused(&r);
    env.att.warn_refused(&r);
    assert_eq!(env.warnings().len(), 1);
}

#[test]
fn months_and_retry_after() {
    let t = |s: &str| s.parse::<Timestamp>().unwrap();
    assert_eq!(month_of(t("2026-11-30T23:59:59Z")), "2026-11");
    assert_eq!(seconds_to_next_month(t("2026-11-30T23:59:59Z")), 1);
    assert_eq!(seconds_to_next_month(t("2026-12-31T00:00:00Z")), 86_400);
}

#[test]
fn config_limits() {
    cfg().validate().unwrap();
    for bad in [
        AttachmentsConfig {
            max_upload_bytes: MAX_UPLOAD_BYTES + 1,
            ..cfg()
        },
        AttachmentsConfig {
            gc_interval_mins: 59,
            ..cfg()
        },
        AttachmentsConfig {
            store: StoreKind::R2,
            ..cfg()
        },
        AttachmentsConfig {
            gc_retention_days: 0,
            ..cfg()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}
