//! Invented data for the screenshot gallery and the tests: a Saturday morning with a busy My Day.
//! Nothing here is real. Built through `decide` and `apply`, like any other state.
//!
//! The clock is fixed at Saturday 21 November 2026, 10:35 local time (UTC+1).

use crate::model::Now;
use jiff::Timestamp;
use jiff::civil::Date;
use todo_core::command::{NewTask, Placement, TaskPatch};
use todo_core::ids::{
    AttachmentId, CollectionId, LinkId, ListId, ReminderId, StepId, TagId, TaskId,
};
use todo_core::model::{EmailLink, LinkKind, Priority};
use todo_core::recurrence::Recurrence;
use todo_core::{Actor, Command, State, apply, decide};
use uuid::Uuid;

/// The owner's device in the web app.
pub const DEVICE: &str = "web";

/// The UTC offset of the fixtures (CET).
pub const OFFSET_S: i32 = 3600;

/// "Now": Saturday 21 November 2026, 10:35 CET.
#[must_use]
pub fn now() -> Now {
    Now {
        at: at("2026-11-21T09:35:00Z"),
        offset_s: OFFSET_S,
        browser_time: false,
    }
}

fn at(s: &str) -> Timestamp {
    s.parse().unwrap_or(Timestamp::UNIX_EPOCH)
}

fn day(d: i8) -> Date {
    Date::new(2026, 11, d).unwrap_or_default()
}

fn uuid(kind: u128, n: u128) -> Uuid {
    // A fixed version-7 shape, so the IDs look like real ones.
    Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0000 | (kind << 48) | n)
}

/// A fixture task ID.
#[must_use]
pub fn task(n: u128) -> TaskId {
    TaskId::from_uuid(uuid(1, n))
}

fn list(n: u128) -> ListId {
    ListId::from_uuid(uuid(2, n))
}

fn coll(n: u128) -> CollectionId {
    CollectionId::from_uuid(uuid(3, n))
}

fn step(n: u128) -> StepId {
    StepId::from_uuid(uuid(4, n))
}

fn tag(n: u128) -> TagId {
    TagId::from_uuid(uuid(5, n))
}

/// A fixture that doesn't apply is a bug in the fixtures: the tests fail on it; the gallery logs it.
fn refused(why: &str) {
    #[cfg(test)]
    panic!("fixture {why}");
    #[cfg(not(test))]
    tracing::warn!("fixture {why}");
}

struct Builder {
    state: State,
    at: Timestamp,
    owner: Actor,
}

impl Builder {
    fn run(&mut self, cmd: Command) {
        self.at = self
            .at
            .checked_add(jiff::SignedDuration::from_secs(1))
            .unwrap_or(self.at);
        match decide(&self.state, &self.owner, &cmd, self.at) {
            Ok(events) => {
                for e in &events {
                    if let Err(err) = apply(&mut self.state, e) {
                        refused(&format!("event not applied: {err}"));
                    }
                }
            }
            Err(err) => refused(&format!("command refused: {err}: {cmd:?}")),
        }
    }

    fn at(&mut self, s: &str) {
        self.at = at(s);
    }

    fn by(&mut self, id: TaskId, agent: &str) {
        if let (Some(t), Ok(a)) = (self.state.tasks.get_mut(&id), Actor::agent(agent)) {
            t.created.by = a.clone();
            t.updated.by = a;
        }
    }
}

struct T<'a> {
    n: u128,
    list: u128,
    title: &'a str,
    my_day: bool,
    due: Option<i8>,
    priority: Priority,
    tags: &'a [u128],
}

const fn t(n: u128, list: u128, title: &str) -> T<'_> {
    T {
        n,
        list,
        title,
        my_day: false,
        due: None,
        priority: Priority::None,
        tags: &[],
    }
}

impl<'a> T<'a> {
    const fn today(mut self) -> Self {
        self.my_day = true;
        self
    }
    const fn due(mut self, d: i8) -> Self {
        self.due = Some(d);
        self
    }
    const fn p(mut self, p: Priority) -> Self {
        self.priority = p;
        self
    }
    const fn tags(mut self, tags: &'a [u128]) -> Self {
        self.tags = tags;
        self
    }
}

/// The older edit conflicts with a later change and cannot be undone without force.
#[cfg(feature = "fixtures")]
pub const REFUSED_ACTIVITY_SEQ: u64 = 101;

/// A deterministic history, separate from the task fixtures so it also covers withdrawn items.
#[cfg(feature = "fixtures")]
pub fn activity() -> Vec<todo_client::activity::ActivityItem> {
    use todo_client::{activity::ActivityItem, protocol::LogEvent};
    use todo_core::{Event, event::EventPayload, ids::EventId};

    let owner = Actor::owner(DEVICE).unwrap_or(Actor::System);
    let codex = Actor::agent("codex").unwrap_or(Actor::System);
    let claude = Actor::agent("claude").unwrap_or(Actor::System);
    let item = |seq: u64, actor, at, summary: &str, undone| {
        let command_id = uuid(6, u128::from(seq));
        let id = EventId::from_uuid(command_id);
        ActivityItem {
            event: LogEvent {
                event: Event {
                    id,
                    seq: Some(seq),
                    group: id,
                    actor,
                    at,
                    undoes: None,
                    forced: false,
                    payload: EventPayload::Updated {
                        target: task(1).into(),
                        changes: Vec::new(),
                    },
                },
                command_id,
                command_idx: 0,
                command_len: Some(1),
            },
            summary: summary.to_owned(),
            undone,
            can_undo: !undone,
        }
    };
    let mut items = vec![
        item(
            104,
            owner.clone(),
            at("2026-11-21T09:29:00Z"),
            "Completed task \"Check the practice room booking\"",
            false,
        ),
        item(
            103,
            codex.clone(),
            at("2026-11-21T09:10:00Z"),
            "Created task \"Draft the volunteer rota\"",
            false,
        ),
        item(
            102,
            Actor::EmailIngest,
            at("2026-11-21T08:50:00Z"),
            "Created task \"Museum tickets for Saturday\"",
            false,
        ),
        item(
            REFUSED_ACTIVITY_SEQ,
            claude.clone(),
            at("2026-11-21T08:23:00Z"),
            "Updated task \"Check the practice room booking\" (due date)",
            false,
        ),
        item(
            100,
            owner,
            at("2026-11-20T17:10:00Z"),
            "Restored list \"Weekend errands\"",
            false,
        ),
        item(
            99,
            codex.clone(),
            at("2026-11-20T15:12:00Z"),
            "Created task \"Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení\"",
            true,
        ),
        item(
            98,
            claude,
            at("2026-11-20T13:20:00Z"),
            "Updated task \"Borrow a music stand\" (notes)",
            false,
        ),
    ];
    for n in (1..=24).rev() {
        let when = at("2026-11-19T08:00:00Z")
            .checked_add(jiff::SignedDuration::from_mins(
                i64::try_from(n).unwrap_or_default(),
            ))
            .unwrap_or_default();
        items.push(item(
            n,
            codex.clone(),
            when,
            &format!("Created task \"Catalogue inventory, shelf {n}\""),
            false,
        ));
    }
    items
}

/// Trash over the gallery state, including a task and list that each need a parent restored first.
#[cfg(feature = "fixtures")]
pub fn trash_state() -> State {
    let mut b = Builder {
        state: state(),
        at: at("2026-11-12T09:00:00Z"),
        owner: Actor::owner(DEVICE).unwrap_or(Actor::System),
    };
    b.run(todo_client::intent::create_collection(
        coll(100),
        "Spring fair",
    ));
    b.run(todo_client::intent::create_list(
        list(100),
        Some(coll(100)),
        "Flyers",
    ));
    for (n, list, title) in [
        (100, list(100), "Send the first draft to print"),
        (101, list(3), "Return the paint samples"),
    ] {
        b.run(Command::CreateTask(Box::new(NewTask::new(
            task(n),
            list,
            title,
        ))));
    }
    for (id, at) in [
        (task(100).into(), "2026-11-17T09:00:00Z"),
        (list(100).into(), "2026-11-18T09:00:00Z"),
        (coll(100).into(), "2026-11-19T09:00:00Z"),
        (task(101).into(), "2026-11-21T09:20:00Z"),
    ] {
        b.at(at);
        b.run(Command::Delete { id });
    }
    b.state
}

/// Tasks around local midnight before Prague's autumn clock change.
#[cfg(feature = "fixtures")]
pub fn fallback_state() -> State {
    let mut b = Builder {
        state: State::new(),
        at: at("2026-10-20T12:00:00Z"),
        owner: Actor::owner(DEVICE).unwrap_or(Actor::System),
    };
    b.run(todo_client::intent::create_list(
        list(1),
        None,
        "Clock change",
    ));
    for (n, title, stamp, done) in [
        (1, "Before midnight", "2026-10-24T21:30:00Z", true),
        (2, "After midnight", "2026-10-24T22:30:00Z", true),
        (3, "Newer agent change", "2026-10-25T08:00:00Z", false),
    ] {
        b.run(Command::CreateTask(Box::new(NewTask::new(
            task(n),
            list(1),
            title,
        ))));
        b.by(task(n), "codex");
        if let Some(t) = b.state.tasks.get_mut(&task(n)) {
            t.updated.at = at(stamp);
            t.completed_at = done.then(|| at(stamp));
        }
    }
    b.state
}

/// The gallery's state: two collections, a few loose lists, a My Day with 12 tasks (3 done).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn state() -> State {
    let mut b = Builder {
        state: State::new(),
        at: at("2026-10-01T08:00:00Z"),
        owner: Actor::owner(DEVICE).unwrap_or(Actor::System),
    };
    let mk_coll = |b: &mut Builder, n, name: &str, color: &str| {
        b.run(Command::CreateCollection {
            id: coll(n),
            name: name.into(),
            color: Some(color.into()),
            placement: Placement::Last,
        });
    };
    let mk_list = |b: &mut Builder, n, c: Option<u128>, name: &str, color: Option<&str>| {
        b.run(Command::CreateList {
            id: list(n),
            collection: c.map(coll),
            name: name.into(),
            color: color.map(Into::into),
            placement: Placement::Last,
        });
    };
    mk_list(&mut b, 1, None, "Inbox", Some("lagoon"));
    mk_coll(&mut b, 1, "Work", "azure");
    mk_list(&mut b, 10, Some(1), "Planning", Some("cornflower"));
    mk_list(&mut b, 11, Some(1), "Finance", Some("clay"));
    mk_list(&mut b, 12, Some(1), "Team", Some("teal"));
    mk_coll(&mut b, 2, "Osobní", "fern");
    mk_list(&mut b, 20, Some(2), "Domácnost", Some("olive"));
    mk_list(&mut b, 21, Some(2), "Nákupy", Some("coral"));
    mk_list(&mut b, 22, Some(2), "Zdraví", Some("rose"));
    mk_list(&mut b, 2, None, "Reading list", Some("orchid"));
    mk_list(&mut b, 3, None, "Garden", None);

    for (n, name) in [
        (1, "quarterly"),
        (2, "finance"),
        (3, "ops"),
        (4, "rodina"),
        (5, "auto"),
        (6, "docs"),
    ] {
        b.run(Command::CreateTag {
            id: tag(n),
            name: name.into(),
            color: None,
        });
    }

    let tasks = [
        // My Day, open.
        t(1, 10, "Prepare the November board update")
            .today()
            .due(21)
            .p(Priority::Normal)
            .tags(&[1, 6]),
        t(2, 11, "Pay the December supplier invoices")
            .today()
            .due(19)
            .p(Priority::High)
            .tags(&[2]),
        t(3, 12, "Review the API latency report").today().tags(&[3]),
        t(4, 20, "Zavolat do servisu kvůli STK")
            .today()
            .due(21)
            .tags(&[5]),
        t(5, 21, "Koupit dárek pro babičku")
            .today()
            .due(23)
            .tags(&[4]),
        t(6, 12, "Draft the agenda for Tuesday’s sync")
            .today()
            .p(Priority::Low),
        t(7, 22, "Objednat se na preventivní prohlídku")
            .today()
            .due(22),
        t(8, 12, "Rotate the backup encryption keys")
            .today()
            .p(Priority::High),
        t(9, 20, "Uklidit sklep před zimou").today(),
        // My Day, done (completed below).
        t(10, 20, "Zalít květiny").today(),
        t(11, 12, "Reply to Ondřej about the offsite").today(),
        t(12, 11, "Zaplatit nájem").today().p(Priority::Normal),
        // Elsewhere.
        t(20, 20, "Připravit nedělní oběd").due(22),
        t(21, 11, "Submit the October expense report").due(24),
        t(22, 1, "Book train tickets to Brno").due(25),
        t(23, 12, "Write up the retro notes").due(26),
        t(24, 1, "Look into a standing desk"),
        t(25, 1, "Find a tailor for the winter coat"),
        t(26, 2, "Finish the field guide to mosses"),
        t(27, 2, "Přečíst Bílou nemoc"),
        t(28, 10, "Outline the 2027 hiring plan")
            .p(Priority::High)
            .due(30),
        t(29, 21, "Žárovky do koupelny"),
        t(30, 20, "Plant the tulip bulbs"),
        t(31, 22, "Vyzvednout recept na brýle"),
        t(32, 1, "Archive old project folders"),
    ];
    // The board update was created a week ago (from an email); the rest over the past days.
    b.at("2026-11-14T08:41:00Z");
    for task_def in &tasks {
        let mut nt = NewTask::new(task(task_def.n), list(task_def.list), task_def.title);
        nt.my_day = task_def.my_day.then(|| day(21));
        nt.due = task_def.due.map(day);
        nt.priority = task_def.priority;
        nt.tags = task_def.tags.iter().copied().map(tag).collect();
        b.run(Command::CreateTask(Box::new(nt)));
        if task_def.n == 1 {
            b.at("2026-11-19T07:00:00Z");
        }
    }

    // Agents created a few.
    b.by(task(3), "codex");
    b.by(task(6), "claude");
    b.by(task(8), "pi");
    b.by(task(23), "claude");

    // The board update: steps, an email source, a reminder, a weekly repeat, an attachment.
    for (n, title) in [
        (1, "Collect numbers from finance"),
        (2, "Update the roadmap slide"),
        (3, "Ask Jana for the hiring summary"),
        (4, "Write the risks section"),
        (5, "Send the draft for review"),
    ] {
        b.run(Command::CreateStep {
            id: step(n),
            task: task(1),
            title: title.into(),
            placement: Placement::Last,
        });
    }
    b.run(Command::CompleteStep { id: step(1) });
    b.run(Command::CompleteStep { id: step(2) });
    b.run(Command::AddLink {
        id: LinkId::from_uuid(uuid(6, 1)),
        task: task(1),
        kind: LinkKind::Email(EmailLink {
            subject: "Board pack — November".into(),
            sender: "petra@example.com".into(),
            received_at: at("2026-11-14T08:12:00Z"),
            message_id: "<board-pack@example.com>".into(),
            web_link: Some("https://example.com/mail/board-pack".into()),
        }),
    });
    b.run(Command::AddAttachment {
        id: AttachmentId::from_uuid(uuid(7, 1)),
        task: task(1),
        file_name: "board-pack-draft.pdf".into(),
        media_type: "application/pdf".into(),
        size_bytes: 1_288_490,
        storage_key: "fixtures/board-pack-draft.pdf".into(),
    });
    b.run(Command::AddReminder {
        id: ReminderId::from_uuid(uuid(8, 1)),
        task: task(1),
        at: at("2026-11-21T14:30:00Z"),
    });
    if let Ok(r) = Recurrence::from_rrule("FREQ=WEEKLY;BYDAY=MO") {
        b.run(Command::UpdateTask {
            id: task(1),
            patch: TaskPatch {
                recurrence: Some(Some(r)),
                ..TaskPatch::default()
            },
        });
    }
    b.run(Command::AddLink {
        id: LinkId::from_uuid(uuid(6, 2)),
        task: task(2),
        kind: LinkKind::Email(EmailLink {
            subject: "Invoices due — December run".into(),
            sender: "accounts@example.com".into(),
            received_at: at("2026-11-18T15:48:00Z"),
            message_id: "<invoices@example.com>".into(),
            web_link: None,
        }),
    });
    b.run(Command::AddLink {
        id: LinkId::from_uuid(uuid(6, 3)),
        task: task(22),
        kind: LinkKind::Web {
            url: "https://example.com/trains".into(),
            title: Some("Timetable".into()),
        },
    });
    for (n, t_n, title) in [
        (10, 3, "Compare p50 and p99"),
        (11, 3, "Flag the slow endpoints"),
        (12, 3, "Post a summary"),
        (13, 9, "Vynést staré krabice"),
        (14, 9, "Zamést"),
        (15, 9, "Srovnat nářadí"),
        (16, 9, "Zkontrolovat vlhkost"),
    ] {
        b.run(Command::CreateStep {
            id: step(n),
            task: task(t_n),
            title: title.into(),
            placement: Placement::Last,
        });
    }
    b.run(Command::CompleteStep { id: step(13) });
    b.run(Command::AddNote {
        task: task(1),
        text: "Keep it to two pages. Lead with the hiring numbers, then the roadmap.".into(),
    });

    // This morning: three done, one edit.
    b.at("2026-11-21T07:48:00Z");
    for n in [10, 12, 11] {
        b.run(Command::CompleteTask { id: task(n) });
    }
    b.at("2026-11-21T08:12:00Z");
    b.run(Command::UpdateTask {
        id: task(1),
        patch: TaskPatch {
            title: Some("Prepare the November board update".into()),
            notes: Some(
                "Keep it to two pages. Lead with the hiring numbers, then the roadmap.\n\nPetra wants it by Monday noon."
                    .into(),
            ),
            ..TaskPatch::default()
        },
    });
    // Some older completed work elsewhere.
    b.at("2026-11-20T16:05:00Z");
    for n in [32, 29] {
        b.run(Command::CompleteTask { id: task(n) });
    }

    // Two agents have worked today; Pi was only present earlier in the week.
    for (n, agent) in [(3, "codex"), (6, "claude")] {
        if let (Some(task), Ok(actor)) = (b.state.tasks.get_mut(&task(n)), Actor::agent(agent)) {
            task.updated.at = at("2026-11-21T09:00:00Z");
            task.updated.by = actor;
        }
    }
    b.state
}

/// Crowded row metadata and the same long prose as the desktop wide-detail fixture.
#[must_use]
#[cfg(any(test, all(target_arch = "wasm32", feature = "fixtures")))]
pub fn wide_detail_state() -> State {
    let mut b = Builder {
        state: state(),
        at: at("2026-11-21T09:00:00Z"),
        owner: Actor::owner(DEVICE).unwrap_or(Actor::System),
    };
    b.run(Command::UpdateList {
        id: list(10),
        name: Some("Planning the quarterly roadmap and every review meeting".into()),
        color: None,
    });
    b.run(Command::UpdateTask {
        id: task(1),
        patch: TaskPatch {
            title: Some("Prepare the quarterly roadmap review and share the proposed milestones with the teams responsible for delivering them".into()),
            notes: Some("Keep the summary to one page so everyone can read it before the review. Compare the shipped work with the original goals and write down where the dates changed, why they changed, and what we learned. Leave room for questions that need a decision before the next planning round.".into()),
            ..TaskPatch::default()
        },
    });
    b.run(Command::UpdateStep {
        id: step(3),
        title: "Check the proposed milestones with each project lead and record the decisions we still need before the next planning round".into(),
    });
    b.run(Command::AddTag {
        task: task(1),
        tag: tag(3),
    });
    b.state
}

/// A quiet agent followed in ID order by five active agents, one more than the sidebar can show.
#[must_use]
#[cfg(any(test, all(target_arch = "wasm32", feature = "fixtures")))]
pub fn agent_rows_state() -> State {
    let mut s = state();
    let yesterday = at("2026-11-20T22:59:59Z");
    let today = at("2026-11-20T23:00:00Z"); // Midnight at the fixture's UTC+1 offset.
    let later = at("2026-11-21T09:15:00Z");
    for (n, agent, created, updated) in [
        (3, "atlas", yesterday, yesterday),
        (4, "claude", yesterday, today),
        (5, "codex", today, later),
        (6, "helper-for-release-notes", today, today),
        (7, "reviewer", today, today),
        (8, "scribe", today, today),
    ] {
        if let (Some(task), Ok(by)) = (s.tasks.get_mut(&task(n)), Actor::agent(agent)) {
            task.created = todo_core::model::Stamp {
                at: created,
                by: by.clone(),
            };
            task.updated = todo_core::model::Stamp { at: updated, by };
        }
    }
    s
}

/// The same morning, but everything in My Day is done: the dusk scene.
#[must_use]
pub fn finished_state() -> State {
    let mut s = state();
    let owner = Actor::owner(DEVICE).unwrap_or(Actor::System);
    let mut at = at("2026-11-21T09:20:00Z");
    for n in 1..=9 {
        at = at
            .checked_add(jiff::SignedDuration::from_secs(60))
            .unwrap_or(at);
        let cmd = Command::CompleteTask { id: task(n) };
        if let Ok(events) = decide(&s, &owner, &cmd, at) {
            for e in &events {
                let _ = apply(&mut s, e);
            }
        }
    }
    s
}

/// A finished day with tasks later this week, but none due tomorrow.
#[must_use]
#[cfg(any(test, all(target_arch = "wasm32", feature = "fixtures")))]
pub fn finished_without_tomorrow() -> State {
    let mut s = finished_state();
    for t in s.tasks.values_mut().filter(|t| t.due == Some(day(22))) {
        t.due = None;
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn every_fixture_command_is_accepted() {
        let s = state();
        assert_eq!(s.lists.len(), 9);
        assert_eq!(s.collections.len(), 2);
        assert_eq!(s.tasks.len(), 25);
        assert_eq!(s.steps.len(), 12);
        assert!(s.tasks[&task(1)].recurrence.is_some());
        assert_eq!(s.tasks[&task(3)].created.by, Actor::agent("codex").unwrap());
    }
}
