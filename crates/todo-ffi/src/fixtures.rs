//! Invented sample data for the screenshot gallery and the tests: made-up tasks, lists and agents, dated
//! relative to the fixture's "now" so the labels read like the F2 mockups ("2d overdue", "in 1h 25m").
//! Never real data.

use crate::labels::Now;
use todo_core::State;
use todo_core::actor::Actor;
use todo_core::ids::{CollectionId, LinkId, ListId, ReminderId, StepId, TagId, TaskId};
use todo_core::model::{
    Collection, EmailLink, Entity, Link, LinkKind, List, ListFeatures, ListView, Priority,
    Reminder, Stamp, Step, Tag, Task, WorkflowStatus,
};
use todo_core::ordering::PosKey;
use todo_core::time::{Date, Timestamp, add_days};

/// Which world to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    /// Three collections, a dozen tasks in My Day (3 done), agents, steps and a mail source.
    World,
    /// The same world with every My Day task done: the dusk scene.
    Finished,
    /// Several tombstones with and without live parents.
    Trash,
    /// Task detail with three activity lines and distinct actors.
    TaskActivity,
    /// The phone header's zero-progress list, or the same list with work done for reach.
    Groceries { started: bool },
    /// Workflow project with all four board columns and date spans.
    Workflow,
    /// The same Markdown source as the desktop and web notes galleries.
    Notes { long_code: bool },
    /// Nothing at all: the first start after pairing.
    Blank,
}

impl Fixture {
    /// Finds a gallery fixture by name.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "world" => Some(Self::World),
            "finished" => Some(Self::Finished),
            "trash" => Some(Self::Trash),
            "task-activity" => Some(Self::TaskActivity),
            "groceries" => Some(Self::Groceries { started: false }),
            "groceries-reach" => Some(Self::Groceries { started: true }),
            "workflow" => Some(Self::Workflow),
            "notes-markdown" => Some(Self::Notes { long_code: false }),
            "notes-code" => Some(Self::Notes { long_code: true }),
            "blank" => Some(Self::Blank),
            _ => None,
        }
    }
}

struct Builder<'a> {
    state: State,
    now: &'a Now,
    today: Date,
    seq: u64,
    pos: PosKey,
    owner: Actor,
}

/// A task's optional parts.
#[derive(Default)]
struct Spec<'a> {
    due: Option<i64>,
    starts: Option<i64>,
    status: WorkflowStatus,
    /// Minutes from now of a reminder (on the due day).
    remind_in_min: Option<i64>,
    priority: Option<Priority>,
    my_day: bool,
    done: bool,
    agent: Option<&'a str>,
    steps: &'a [(&'a str, bool)],
    tags: &'a [TagId],
    notes: &'a str,
    mail: Option<(&'a str, &'a str, i64)>,
    web_link: Option<&'a str>,
}

impl<'a> Builder<'a> {
    fn new(now: &'a Now) -> Self {
        Self {
            state: State::new(),
            now,
            today: now.today(),
            seq: 0,
            pos: PosKey::first(),
            // A fixed device ID, as the server would issue one.
            owner: Actor::owner("fixture-phone").unwrap_or(Actor::System),
        }
    }

    fn seed(&mut self, kind: &str) -> (Timestamp, Vec<u8>) {
        self.seq += 1;
        (
            self.now.at,
            format!("fixture/{kind}/{}", self.seq).into_bytes(),
        )
    }

    fn stamp(&self, minutes_ago: i64, by: &Actor) -> Stamp {
        Stamp {
            at: Timestamp::from_second(self.now.at.as_second() - minutes_ago * 60)
                .unwrap_or(self.now.at),
            by: by.clone(),
        }
    }

    fn next_pos(&mut self) -> PosKey {
        let p = self.pos.clone();
        self.pos = self.pos.after().unwrap_or_else(|_| PosKey::first());
        p
    }

    fn insert(&mut self, e: Entity) {
        // Fresh IDs and existing parents: inserting can't fail.
        let _ = self.state.insert(e);
    }

    fn collection(&mut self, name: &str, color: &str) -> CollectionId {
        let (at, seed) = self.seed("collection");
        let id = CollectionId::derive(at, &seed);
        let position = self.next_pos();
        let created = self.stamp(60 * 24 * 30, &self.owner.clone());
        self.insert(Entity::Collection(Collection {
            task_sort: None,
            id,
            name: name.into(),
            color: Some(color.into()),
            position,
            created,
            deleted: None,
        }));
        id
    }

    fn list(&mut self, collection: Option<CollectionId>, name: &str, color: &str) -> ListId {
        let (at, seed) = self.seed("list");
        let id = ListId::derive(at, &seed);
        let position = self.next_pos();
        let created = self.stamp(60 * 24 * 30, &self.owner.clone());
        self.insert(Entity::List(List {
            task_sort: None,
            features: Default::default(),
            id,
            collection,
            name: name.into(),
            color: Some(color.into()),
            position,
            created,
            deleted: None,
        }));
        id
    }

    fn tag(&mut self, name: &str) -> TagId {
        let (at, seed) = self.seed("tag");
        let id = TagId::derive(at, &seed);
        let created = self.stamp(60 * 24 * 30, &self.owner.clone());
        self.insert(Entity::Tag(Tag {
            id,
            name: name.into(),
            color: None,
            created,
            deleted: None,
        }));
        id
    }

    fn task(&mut self, list: ListId, title: &str, spec: Spec<'_>) -> TaskId {
        let (at, seed) = self.seed("task");
        let id = TaskId::derive(at, &seed);
        let by = spec
            .agent
            .and_then(|a| Actor::agent(a).ok())
            .unwrap_or_else(|| self.owner.clone());
        let created = self.stamp(60 * 24 * 9 + i64::try_from(self.seq).unwrap_or(0), &by);
        let updated = self.stamp(45, &by);
        let due = spec.due.and_then(|d| add_days(self.today, d));
        let position = self.next_pos();
        self.insert(Entity::Task(Task {
            id,
            list,
            title: title.into(),
            notes: spec.notes.into(),
            due,
            start_date: spec.starts.and_then(|d| add_days(self.today, d)),
            status: spec.status,
            priority: spec.priority.unwrap_or_default(),
            my_day: spec.my_day.then_some(self.today),
            completed_at: spec.done.then(|| self.stamp(30, &by).at),
            recurrence: None,
            tags: spec.tags.iter().copied().collect(),
            position,
            created,
            updated,
            deleted: None,
            merged_into: None,
        }));
        for (title, done) in spec.steps {
            let (at, seed) = self.seed("step");
            let position = self.next_pos();
            let created = self.stamp(60 * 24, &self.owner.clone());
            self.insert(Entity::Step(Step {
                id: StepId::derive(at, &seed),
                task: id,
                title: (*title).into(),
                completed_at: done.then_some(self.now.at),
                position,
                created,
                deleted: None,
            }));
        }
        if let Some(m) = spec.remind_in_min {
            let (at, seed) = self.seed("reminder");
            let when = Timestamp::from_second(self.now.at.as_second() + m * 60).unwrap_or(at);
            let created = self.stamp(60, &self.owner.clone());
            self.insert(Entity::Reminder(Reminder {
                id: ReminderId::derive(at, &seed),
                task: id,
                at: when,
                created,
                deleted: None,
            }));
        }
        if let Some((subject, sender, days_ago)) = spec.mail {
            let (at, seed) = self.seed("link");
            let received_at =
                Timestamp::from_second(self.now.at.as_second() - days_ago * 86_400).unwrap_or(at);
            let created = self.stamp(60 * 24 * days_ago, &Actor::EmailIngest);
            self.insert(Entity::Link(Link {
                id: LinkId::derive(at, &seed),
                task: id,
                kind: LinkKind::Email(EmailLink {
                    subject: subject.into(),
                    sender: sender.into(),
                    received_at,
                    message_id: format!("<fixture-{}@example.com>", self.seq),
                    web_link: spec.web_link.map(str::to_owned),
                }),
                created,
                deleted: None,
            }));
        }
        id
    }
}

fn task_activity(mut b: Builder<'_>) -> State {
    use todo_core::command::{NewTask, TaskPatch};
    use todo_core::{AccessLevel, Command, Grants, apply, decide};

    let list = b.list(None, "Design", "azure");
    let (at, seed) = b.seed("task");
    let id = TaskId::derive(at, &seed);
    let codex = Actor::agent("codex").unwrap_or(Actor::System);
    let pi = Actor::agent("pi").unwrap_or(Actor::System);
    for agent in [codex.agent_id(), pi.agent_id()].into_iter().flatten() {
        b.state.grants.insert(
            agent.clone(),
            Grants {
                default: AccessLevel::Plan,
                ..Grants::default()
            },
        );
    }
    for (minutes_ago, by, cmd) in [
        (
            2 * 24 * 60,
            codex,
            Command::CreateTask(Box::new(NewTask::new(id, list, "Review the color proofs"))),
        ),
        (90, pi, Command::CompleteTask { id }),
        (
            12,
            b.owner.clone(),
            Command::UpdateTask {
                id,
                patch: TaskPatch {
                    notes: Some("The warm option is approved.".into()),
                    ..TaskPatch::default()
                },
            },
        ),
    ] {
        if let Ok(events) = decide(&b.state, &by, &cmd, b.stamp(minutes_ago, &by).at) {
            for event in &events {
                let _ = apply(&mut b.state, event);
            }
        }
    }
    b.state
}

fn groceries(mut b: Builder<'_>, started: bool) -> State {
    let home = b.collection("Home", "teal");
    let list = b.list(Some(home), "Groceries", "teal");
    b.list(Some(home), "Chores", "rose");
    b.list(Some(home), "Repairs", "olive");
    let bakery = b.tag("bakery");
    let pantry = b.tag("pantry");
    b.task(
        list,
        "Buy milk and sourdough bread",
        Spec {
            due: Some(0),
            remind_in_min: Some(445),
            tags: &[bakery],
            ..Spec::default()
        },
    );
    for title in [
        "Pick up fresh basil and parsley",
        "Order whole-bean coffee",
        "Oat milk (two cartons)",
        "Dishwasher tablets",
        "Olive oil and sea salt",
        "Brown rice",
        "Apples",
        "Tomato paste",
        "Tea",
    ] {
        b.task(
            list,
            title,
            Spec {
                tags: &[pantry],
                ..Spec::default()
            },
        );
    }
    for title in [
        "Lemons",
        "Dried apricots",
        "Paper towels",
        "Red lentils",
        "Black pepper",
    ] {
        b.task(
            list,
            title,
            Spec {
                done: started,
                ..Spec::default()
            },
        );
    }
    b.state
}

/// Builds a fixture world as of `now`.
fn workflow(mut b: Builder<'_>) -> State {
    let home = b.collection("Home", "coral");
    let renovation = b.list(Some(home), "Home renovation", "coral");
    b.list(Some(home), "Groceries", "teal");
    if let Some(list) = b.state.lists.get_mut(&renovation) {
        list.features = ListFeatures {
            workflow: true,
            timeline: true,
            default_view: ListView::Board,
        };
    }
    let lighting = b.tag("lighting");
    let carpentry = b.tag("carpentry");
    let hallway = b.tag("hallway");
    let bathroom = b.tag("bathroom");
    let permits = b.tag("permits");
    b.task(
        renovation,
        "Compare kitchen pendant lights",
        Spec {
            status: WorkflowStatus::Backlog,
            agent: Some("pi"),
            tags: &[lighting],
            ..Spec::default()
        },
    );
    b.task(
        renovation,
        "Get a quote for the hallway wardrobe",
        Spec {
            status: WorkflowStatus::Backlog,
            starts: Some(12),
            due: Some(16),
            tags: &[carpentry],
            ..Spec::default()
        },
    );
    b.task(
        renovation,
        "Order matte oak flooring samples",
        Spec {
            starts: Some(2),
            due: Some(4),
            priority: Some(Priority::Normal),
            agent: Some("claude"),
            steps: &[
                ("Compare finishes", true),
                ("Check stock", false),
                ("Order samples", false),
            ],
            ..Spec::default()
        },
    );
    b.task(
        renovation,
        "Confirm electrician visit for kitchen outlets",
        Spec {
            due: Some(3),
            priority: Some(Priority::High),
            ..Spec::default()
        },
    );
    b.task(
        renovation,
        "Measure bathroom vanity niche and mirror",
        Spec {
            starts: Some(5),
            due: Some(6),
            steps: &[
                ("Measure the opening", false),
                ("Measure the mirror", false),
            ],
            ..Spec::default()
        },
    );
    b.task(renovation, "Strip wallpaper and prime hallway walls", Spec {
        status: WorkflowStatus::InProgress, starts: Some(-2), due: Some(2), priority: Some(Priority::High), tags: &[hallway],
        notes: "Use a water-based primer. Keep the samples by the window to see them in daylight.",
        steps: &[("Cover the floor", true), ("Move the furniture", true), ("Strip the wallpaper", false), ("Prime the walls", false)],
        ..Spec::default()
    });
    b.task(
        renovation,
        "Pick grout color for bathroom tiles",
        Spec {
            status: WorkflowStatus::InProgress,
            due: Some(-1),
            agent: Some("codex"),
            tags: &[bathroom],
            ..Spec::default()
        },
    );
    b.task(
        renovation,
        "Vyřídit ohlášení drobných stavebních úprav na úřadě",
        Spec {
            status: WorkflowStatus::InProgress,
            starts: Some(-1),
            due: Some(5),
            agent: Some("claude"),
            tags: &[permits],
            ..Spec::default()
        },
    );
    for title in [
        "Measure door frames",
        "Order paint samples",
        "Clear the hallway",
    ] {
        b.task(
            renovation,
            title,
            Spec {
                done: true,
                ..Spec::default()
            },
        );
    }
    b.state
}

pub fn build(fixture: Fixture, now: &Now) -> State {
    let mut b = Builder::new(now);
    if fixture == Fixture::Blank {
        return b.state;
    }
    if fixture == Fixture::TaskActivity {
        return task_activity(b);
    }
    if fixture == Fixture::Workflow {
        return workflow(b);
    }
    if let Fixture::Groceries { started } = fixture {
        return groceries(b, started);
    }
    if let Fixture::Notes { long_code } = fixture {
        let notes = if long_code {
            include_str!("../../todo-client/tests/fixtures/notes-code.md")
        } else {
            include_str!("../../todo-client/tests/fixtures/notes-markdown.md")
        };
        let work = b.collection("Work", "lagoon");
        let list = b.list(Some(work), "Q4 planning", "lagoon");
        b.task(
            list,
            "Prepare the November board update",
            Spec {
                notes: notes.trim_end(),
                ..Spec::default()
            },
        );
        return b.state;
    }
    let finished = fixture == Fixture::Finished;
    let my = |done: bool| Spec {
        my_day: true,
        done: done || finished,
        ..Spec::default()
    };

    let work = b.collection("Work", "lagoon");
    let personal = b.collection("Personal", "rose");
    let projects = b.collection("Projects", "jade");

    let finance = b.list(Some(work), "Finance", "clay");
    let events = b.list(Some(work), "Events", "fern");
    let planning = b.list(Some(work), "Planning", "azure");
    let errands = b.list(Some(personal), "Errands", "coral");
    let admin = b.list(Some(personal), "Admin", "orchid");
    let health = b.list(Some(personal), "Health", "pink");
    let travel = b.list(Some(personal), "Travel", "clay");
    let app = b.list(Some(projects), "To-do app", "fern");
    b.list(Some(projects), "Reading list", "jade");

    let home = b.tag("domácnost");
    let trip = b.tag("trip");
    let admin_tag = b.tag("admin");

    // My Day: twelve tasks, three of them done (all twelve when finished).
    b.task(
        finance,
        "Pay quarterly VAT filing",
        Spec {
            due: Some(-2),
            priority: Some(Priority::High),
            ..my(false)
        },
    );
    b.task(
        events,
        "Send venue booking confirmation",
        Spec {
            due: Some(0),
            remind_in_min: Some(85),
            mail: Some(("Venue booking for the offsite", "events@example.com", 2)),
            ..my(false)
        },
    );
    b.task(
        planning,
        "Prepare Q4 roadmap review",
        Spec {
            due: Some(0),
            remind_in_min: Some(385),
            priority: Some(Priority::Normal),
            agent: Some("codex"),
            steps: &[
                ("Collect the team updates", true),
                ("Draft the agenda", true),
                ("Book the room", false),
                ("Prepare the slides", false),
                ("Send the pre-read", false),
            ],
            ..my(false)
        },
    );
    b.task(
        errands,
        "Koupit rohlíky a mléko",
        Spec {
            due: Some(0),
            remind_in_min: Some(475),
            tags: &[home],
            ..my(false)
        },
    );
    b.task(
        app,
        "Review sync-engine pull request",
        Spec {
            due: Some(0),
            priority: Some(Priority::High),
            agent: Some("claude"),
            ..my(false)
        },
    );
    b.task(
        admin,
        "Renew passport appointment",
        Spec {
            due: Some(1),
            remind_in_min: Some(445),
            priority: Some(Priority::Normal),
            agent: Some("pi"),
            steps: &[
                ("Check which documents are required", true),
                ("Book a slot on the town hall site", false),
                ("Get a passport photo taken", false),
                ("Fill in the application form", false),
            ],
            tags: &[admin_tag],
            notes: "Bring the old passport and proof of address. Slots open on Mondays at 08:00.",
            mail: Some(("Your appointment reminder", "info@example.com", 9)),
            web_link: Some("https://example.com/appointment"),
            ..my(false)
        },
    );
    b.task(
        health,
        "Vyřídit žádost o přeplatek na zdravotním pojištění",
        Spec {
            due: Some(2),
            priority: Some(Priority::Low),
            agent: Some("pi"),
            ..my(false)
        },
    );
    b.task(
        app,
        "Write schema migration notes",
        Spec {
            due: Some(2),
            agent: Some("codex"),
            steps: &[
                ("List the changed tables", true),
                ("Describe the backfill", false),
                ("Note the rollback", false),
            ],
            ..my(false)
        },
    );
    let appointment = b.task(
        health,
        "Call the dentist to reschedule",
        Spec { ..my(false) },
    );
    b.task(errands, "Water the balcony plants", my(true));
    b.task(events, "Book a room for the retro", my(true));
    b.task(finance, "Reply to the accountant", my(true));

    // Elsewhere.
    b.task(
        events,
        "Call Alex about the venue",
        Spec {
            due: Some(1),
            // Tomorrow at 15:00.
            remind_in_min: Some(28 * 60 + 25),
            ..Spec::default()
        },
    );
    b.task(
        health,
        "Refill the prescription",
        Spec {
            due: Some(-1),
            priority: Some(Priority::High),
            ..Spec::default()
        },
    );
    b.task(
        errands,
        "Pick up the dry cleaning",
        Spec {
            due: Some(3),
            ..Spec::default()
        },
    );
    b.task(
        travel,
        "Pack the weekend bag",
        Spec {
            due: Some(6),
            priority: Some(Priority::Low),
            steps: &[
                ("Charger", false),
                ("Tickets", false),
                ("Rain jacket", false),
            ],
            ..Spec::default()
        },
    );
    b.task(
        travel,
        "Book train tickets to Brno",
        Spec {
            due: Some(7),
            priority: Some(Priority::Normal),
            agent: Some("claude"),
            tags: &[trip],
            ..Spec::default()
        },
    );
    b.task(
        health,
        "Schedule the annual check-up",
        Spec {
            due: Some(11),
            ..Spec::default()
        },
    );
    b.task(
        errands,
        "Return the library books",
        Spec {
            done: true,
            ..Spec::default()
        },
    );
    b.task(
        planning,
        "Collect feedback on the offsite",
        Spec {
            due: Some(4),
            agent: Some("claude"),
            ..Spec::default()
        },
    );
    for (name, media, bytes) in [
        ("Appointment.pdf", "application/pdf", 32768),
        ("Photo.jpg", "image/jpeg", 1459200),
    ] {
        let (at, seed) = b.seed("attachment");
        b.insert(Entity::Attachment(todo_core::model::AttachmentRef {
            id: todo_core::ids::AttachmentId::derive(at, &seed),
            task: appointment,
            file_name: name.into(),
            media_type: media.into(),
            size_bytes: bytes,
            storage_key: format!("fixture/{name}"),
            created: b.stamp(60, &b.owner),
            deleted: None,
        }));
    }
    if fixture == Fixture::Trash {
        let fair = b.collection("Spring fair", "jade");
        let flyers = b.list(Some(fair), "Flyers", "clay");
        let draft = b.task(flyers, "Send the first draft to print", Spec::default());
        let paint = b.task(errands, "Return the paint samples", Spec::default());
        for (id, minutes) in [
            (draft.into(), 4 * 1440),
            (flyers.into(), 3 * 1440),
            (fair.into(), 2 * 1440),
            (paint.into(), 15),
        ] {
            let stamp = b.stamp(minutes, &b.owner);
            let _ = b.state.set_tombstone(
                id,
                Some(todo_core::model::Tombstone {
                    at: stamp.at,
                    by: stamp.by,
                }),
            );
        }
    }
    b.state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::LocalNow;
    use crate::view::{self, RoomKey, SmartList};

    fn now() -> Now {
        // Saturday 21 Nov 2026, 10:35 in Prague.
        let at: Timestamp = "2026-11-21T09:35:00Z".parse().unwrap();
        Now::new(LocalNow {
            epoch_ms: at.as_millisecond(),
            utc_offset_secs: 3600,
            zone_id: "Europe/Prague".into(),
            local_day: None,
            date_format: None,
        })
    }

    fn my_day() -> RoomKey {
        RoomKey::Smart {
            list: SmartList::MyDay,
        }
    }

    #[test]
    fn the_world_matches_the_cover_mockup() {
        let n = now();
        let s = build(Fixture::World, &n);
        let r = view::room(&s, &my_day(), &n);
        assert_eq!((r.done, r.total, r.open), (3, 12, 9));
        assert_eq!(r.summary_text, "3 of 12 done · 9 open");
        assert!(!r.finished);
        let first = &r.tasks[0];
        assert_eq!(first.title, "Pay quarterly VAT filing");
        assert_eq!(first.due.as_ref().unwrap().text, "2d overdue");
        assert_eq!(first.priority, 3);
        assert_eq!(first.chip.as_ref().unwrap().label, "Finance");
        let venue = r
            .tasks
            .iter()
            .find(|t| t.title.starts_with("Send venue"))
            .unwrap();
        assert_eq!(venue.due.as_ref().unwrap().text, "in 1h 25m");
        assert!(venue.from_mail);
        let roadmap = r
            .tasks
            .iter()
            .find(|t| t.title.starts_with("Prepare Q4"))
            .unwrap();
        assert_eq!(roadmap.due.as_ref().unwrap().text, "Today 17:00");
        assert_eq!(roadmap.agent.as_ref().unwrap().letter, "X");
        assert_eq!(roadmap.steps.unwrap().done, 2);
        if let view::Eyebrow::Day {
            weekday,
            date,
            week,
        } = &r.eyebrow
        {
            assert_eq!(
                (weekday.as_str(), date.as_str(), week.as_str()),
                ("Saturday", "21.11.", "W47")
            );
        } else {
            panic!("a smart list has a day eyebrow");
        }
    }

    #[test]
    fn the_finished_world_hands_over_to_tomorrow() {
        let n = now();
        let s = build(Fixture::Finished, &n);
        let r = view::room(&s, &my_day(), &n);
        assert!(r.finished);
        assert_eq!((r.done, r.total, r.open), (12, 12, 0));
        assert_eq!(
            r.summary,
            view::RoomSummary::AllClear {
                done: 12,
                total: 12
            }
        );
        let t = r.tomorrow.unwrap();
        assert_eq!(t.weekday, "Sunday");
        assert!(!t.week.is_empty());
    }

    #[test]
    fn rooms_and_detail_for_the_inner_mockup() {
        let n = now();
        let s = build(Fixture::World, &n);
        let home = view::home(&s, &n);
        let names: Vec<_> = home
            .groups
            .iter()
            .filter_map(|g| g.collection.as_ref().map(|c| c.name.as_str()))
            .collect();
        assert_eq!(names, ["Work", "Personal", "Projects"]);
        let personal = home.groups[1].collection.clone().unwrap();
        let r = view::room(&s, &personal.key, &n);
        assert_eq!(r.title, "Personal");
        assert!(r.tasks.iter().all(|t| t.chip.is_some()));
        let passport = r
            .tasks
            .iter()
            .find(|t| t.title.starts_with("Renew"))
            .unwrap();
        let d = view::detail(&s, &passport.id, &n).unwrap();
        assert_eq!(d.steps.len(), 4);
        assert_eq!(
            d.breadcrumb
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["Personal", "Admin"]
        );
        assert_eq!(d.sources[0].title, "Your appointment reminder");
        assert!(d.fields.iter().any(|f| f.label == "Reminder"));
    }

    #[test]
    fn the_reading_list_is_empty_with_hills_only() {
        let n = now();
        let s = build(Fixture::World, &n);
        let home = view::home(&s, &n);
        let reading = home
            .groups
            .iter()
            .flat_map(|g| g.lists.iter())
            .find(|l| l.name == "Reading list")
            .unwrap();
        let r = view::room(&s, &reading.key, &n);
        assert!(r.tasks.is_empty());
        assert_eq!(r.total, 0);
        assert_eq!(r.empty_line, "Nothing here yet.");
    }

    #[test]
    fn a_reopening_keeps_its_completed_place_to_the_nanosecond() {
        let n = now();
        let mut s = build(Fixture::World, &n);
        let done: Vec<TaskId> = view::room(&s, &my_day(), &n)
            .completed
            .iter()
            .map(|t| t.id.parse().unwrap())
            .collect();
        assert!(done.len() >= 2);
        // Two tasks finished in the same millisecond, 800 ns apart: the later one sorts first.
        let ms = n.at.as_nanosecond() / 1_000_000 * 1_000_000;
        for (id, ns) in [(done[0], 100), (done[1], 900)] {
            let t = s.tasks.get_mut(&id).unwrap();
            t.completed_at = Some(Timestamp::from_nanosecond(ms + ns).unwrap());
        }
        let before = view::room(&s, &my_day(), &n).completed;
        assert_eq!(before[0].id, done[1].to_string());
        assert_eq!(before[1].id, done[0].to_string());

        // The later one is reopened: while its row leaves, the placement puts it back exactly.
        let row = &before[0];
        let at = row.completed_at.clone().unwrap();
        s.tasks.get_mut(&done[1]).unwrap().completed_at = None;
        let placement = view::RowPlacement {
            holding: vec![],
            reopening: vec![view::Reopening {
                id: row.id.clone(),
                completed_at: at,
            }],
        };
        let kept = view::room(&view::placed(&s, &placement), &my_day(), &n).completed;
        assert_eq!(kept, before);
    }

    #[test]
    fn fixtures_are_deterministic() {
        let n = now();
        assert_eq!(build(Fixture::World, &n), build(Fixture::World, &n));
        assert!(build(Fixture::Blank, &n).tasks.is_empty());
    }
}
