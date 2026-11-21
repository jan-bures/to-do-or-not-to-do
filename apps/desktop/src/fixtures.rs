//! The `--screenshot` fixtures: invented sample data, never real tasks.
//!
//! Each fixture is a `todo_core::State` built by running commands through `decide` and `apply` (as agents
//! and the owner, at fixed times), plus the window's UI state, a fake sync status and a fixed clock. The
//! screenshot renders it through the same derivation (`model::derive`) as the running app, so the gallery
//! shows the real views, not a mock-up of them. No server is involved.

use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::ids::{
    AttachmentId, CollectionId, LinkId, ListId, ReminderId, StepId, TagId, TaskId, uuid_v7,
};
use todo_core::model::{EmailLink, LinkKind, Priority};
use todo_core::recurrence::{Recurrence, Rule};
use todo_core::time::{Date, Timestamp, Weekday};
use todo_core::{AccessLevel, Actor, AgentId, Grants, State, apply, decide};

use crate::model::{Now, RoomKey, Smart, SyncInfo, UiState};

/// A fixture: data, UI state, sync status and clock.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub state: State,
    pub ui: UiState,
    pub sync: SyncInfo,
    pub now: Now,
    /// Text typed into the room's quick-add, to show its focused state.
    pub quick_add_text: Option<&'static str>,
}

impl Fixture {
    #[cfg(test)]
    pub fn shell(&self) -> crate::model::Shell {
        crate::model::derive(&self.state, &self.ui, &self.sync, &self.now)
    }
}

/// The server the pairing fixture shows (a placeholder domain).
pub const PAIRING_SERVER: &str = "https://todo.example.com";
/// The user code the pairing fixture shows.
pub const PAIRING_CODE: &str = "KQ4M-7XWD";

/// The fixture with this name (see `args::FIXTURES`). `quick-add` reuses My Day for its room;
/// `pairing` is drawn by the pairing view and has no shell.
pub fn by_name(name: &str) -> Option<Fixture> {
    match name {
        "my-day" | "quick-add" => Some(my_day()),
        "work" => Some(work()),
        "collapsed-collection" => Some(collapsed_collection()),
        "long-sidebar" => Some(long_sidebar()),
        "long-names" => Some(long_names()),
        "empty" => Some(empty()),
        "finished" => Some(finished()),
        "pairing" | "pairing-code" | "open-failed" | "open-failed-confirm" => Some(pairing()),
        _ => None,
    }
}

/// "Saturday 21 Nov 2026, 10:35", the morning of the F2 mockups.
const MORNING: &str = "2026-11-21T10:35:00Z";

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap_or(Timestamp::UNIX_EPOCH)
}

fn date(s: &str) -> Date {
    s.parse().unwrap_or_default()
}

fn owner() -> Actor {
    Actor::owner("desktop").unwrap_or(Actor::System)
}

fn agent(id: &str) -> Actor {
    Actor::agent(id).unwrap_or(Actor::System)
}

/// The agents in the sample data.
const AGENTS: [&str; 3] = ["claude", "codex", "pi"];

/// Runs commands against a state. A command the domain refuses is recorded, and a test checks that
/// every fixture builds without one.
struct Builder {
    state: State,
    n: u64,
    errors: Vec<String>,
}

impl Builder {
    fn new() -> Self {
        let mut state = State::new();
        // The sample agents may plan everywhere (the server keeps grants; fixtures set them directly).
        for id in AGENTS {
            if let Ok(agent) = AgentId::new(id) {
                state.grants.insert(
                    agent,
                    Grants {
                        default: AccessLevel::Plan,
                        ..Grants::default()
                    },
                );
            }
        }
        Self {
            state,
            n: 0,
            errors: Vec::new(),
        }
    }

    fn uuid(&mut self) -> uuid::Uuid {
        self.n += 1;
        let mut rand = [0u8; 10];
        rand[..8].copy_from_slice(&self.n.to_le_bytes());
        uuid_v7(1_763_000_000_000 + self.n, rand)
    }

    fn run(&mut self, actor: &Actor, at: &str, cmd: Command) {
        let events = match decide(&self.state, actor, &cmd, ts(at)) {
            Ok(events) => events,
            Err(e) => {
                self.errors.push(format!("{cmd:?}: {e}"));
                return;
            }
        };
        for e in &events {
            if let Err(err) = apply(&mut self.state, e) {
                self.errors.push(format!("{cmd:?}: {err}"));
            }
        }
    }

    fn collection(&mut self, name: &str, swatch: &str) -> CollectionId {
        let id = CollectionId::from_uuid(self.uuid());
        self.run(
            &owner(),
            "2026-01-10T09:00:00Z",
            Command::CreateCollection {
                id,
                name: name.into(),
                color: Some(swatch.into()),
                placement: Placement::Last,
            },
        );
        id
    }

    fn list(&mut self, collection: CollectionId, name: &str, swatch: &str) -> ListId {
        let id = ListId::from_uuid(self.uuid());
        self.run(
            &owner(),
            "2026-01-10T09:00:00Z",
            Command::CreateList {
                id,
                collection: Some(collection),
                name: name.into(),
                color: Some(swatch.into()),
                placement: Placement::Last,
            },
        );
        id
    }

    fn tag(&mut self, name: &str) -> TagId {
        let id = TagId::from_uuid(self.uuid());
        self.run(
            &owner(),
            "2026-01-10T09:00:00Z",
            Command::CreateTag {
                id,
                name: name.into(),
                color: None,
            },
        );
        id
    }

    fn task(&mut self, t: T) -> TaskId {
        let id = TaskId::from_uuid(self.uuid());
        // A task an agent edits later starts as a rougher draft of its title.
        let first = match t.edited_by {
            Some(_) => format!("{} (draft)", t.title),
            None => t.title.to_string(),
        };
        let mut new = NewTask::new(id, t.list, first);
        new.priority = t.priority;
        new.due = t.due.map(date);
        new.my_day = t.my_day.then(|| date("2026-11-21"));
        new.tags = t.tags.clone();
        new.notes = t.notes.into();
        if let Some(day) = t.repeat {
            new.recurrence = Some(Recurrence::new(Rule::Weekly {
                interval: 1,
                days: [day].into_iter().collect(),
            }));
        }
        let by = t.by.map_or_else(owner, agent);
        self.run(&by, t.created, Command::CreateTask(Box::new(new)));
        for (i, title) in t.steps.iter().enumerate() {
            let step = StepId::from_uuid(self.uuid());
            self.run(
                &by,
                t.created,
                Command::CreateStep {
                    id: step,
                    task: id,
                    title: (*title).into(),
                    placement: Placement::Last,
                },
            );
            if i < t.steps_done {
                self.run(&owner(), t.created, Command::CompleteStep { id: step });
            }
        }
        if let Some(at) = t.reminder {
            let rid = ReminderId::from_uuid(self.uuid());
            self.run(
                &owner(),
                t.created,
                Command::AddReminder {
                    id: rid,
                    task: id,
                    at: ts(at),
                },
            );
        }
        if let Some((subject, sender, received)) = t.mail {
            let lid = LinkId::from_uuid(self.uuid());
            self.run(
                &Actor::EmailIngest,
                t.created,
                Command::AddLink {
                    id: lid,
                    task: id,
                    kind: LinkKind::Email(EmailLink {
                        subject: subject.into(),
                        sender: sender.into(),
                        received_at: ts(received),
                        message_id: format!("<{}@mail.example.com>", self.n),
                        web_link: None,
                    }),
                },
            );
        }
        for (file_name, media_type, size_bytes) in t.files {
            let aid = AttachmentId::from_uuid(self.uuid());
            self.run(
                &Actor::EmailIngest,
                t.created,
                Command::AddAttachment {
                    id: aid,
                    task: id,
                    file_name: (*file_name).into(),
                    media_type: (*media_type).into(),
                    size_bytes: *size_bytes,
                    storage_key: format!("fixture/{aid}"),
                },
            );
        }
        if let Some((agent_id, at)) = t.edited_by {
            // An agent's later edit: it tidies the title.
            self.run(
                &agent(agent_id),
                at,
                Command::UpdateTask {
                    id,
                    patch: TaskPatch {
                        title: Some(t.title.into()),
                        ..TaskPatch::default()
                    },
                },
            );
        }
        if let Some(at) = t.done {
            self.run(&owner(), at, Command::CompleteTask { id });
        }
        id
    }
}

/// A task to build. Times are UTC strings.
#[derive(Clone)]
struct T {
    list: ListId,
    title: &'static str,
    priority: Priority,
    due: Option<&'static str>,
    reminder: Option<&'static str>,
    my_day: bool,
    tags: Vec<TagId>,
    steps: &'static [&'static str],
    steps_done: usize,
    notes: &'static str,
    repeat: Option<Weekday>,
    by: Option<&'static str>,
    created: &'static str,
    edited_by: Option<(&'static str, &'static str)>,
    mail: Option<(&'static str, &'static str, &'static str)>,
    /// Attachments that came with the email: (file name, media type, size in bytes).
    files: &'static [(&'static str, &'static str, u64)],
    done: Option<&'static str>,
}

fn t(list: ListId, title: &'static str) -> T {
    T {
        list,
        title,
        priority: Priority::None,
        due: None,
        reminder: None,
        my_day: false,
        tags: Vec::new(),
        steps: &[],
        steps_done: 0,
        notes: "",
        repeat: None,
        by: None,
        created: "2026-11-14T08:00:00Z",
        edited_by: None,
        mail: None,
        files: &[],
        done: None,
    }
}

/// Lists and tags of the sample data.
#[allow(dead_code)] // Not every fixture uses every list.
struct Lists {
    planning: ListId,
    finance: ListId,
    office: ListId,
    events: ListId,
    todo_app: ListId,
    home: ListId,
    reading: ListId,
    errands: ListId,
    health: ListId,
    travel: ListId,
    passport: TaskId,
    q4: TaskId,
}

/// The shared sample data: three collections, ten lists, the day's plan, the week ahead and a backlog of
/// finished tasks.
fn sample() -> (Builder, Lists) {
    let mut b = Builder::new();
    let work = b.collection("Work", "lagoon");
    let planning = b.list(work, "Planning", "azure");
    let finance = b.list(work, "Finance", "teal");
    let office = b.list(work, "Office", "cornflower");
    let events = b.list(work, "Events", "jade");
    let projects = b.collection("Projects", "fern");
    let todo_app = b.list(projects, "To-do app", "fern");
    let home = b.list(projects, "Home renovation", "olive");
    let reading = b.list(projects, "Reading list", "jade");
    let personal = b.collection("Personal", "rose");
    let errands = b.list(personal, "Errands", "coral");
    let health = b.list(personal, "Health", "pink");
    let travel = b.list(personal, "Travel", "clay");

    let tag = |b: &mut Builder, names: &[&str]| -> Vec<TagId> {
        names.iter().map(|n| b.tag(n)).collect()
    };
    let tax = tag(&mut b, &["tax"]);
    let q4 = tag(&mut b, &["planning", "q4"]);
    let domacnost = tag(&mut b, &["domácnost"]);
    let review = tag(&mut b, &["review"]);
    let pojisteni = tag(&mut b, &["pojištění"]);
    let docs = tag(&mut b, &["docs"]);
    let retro = tag(&mut b, &["retro"]);
    let budget = tag(&mut b, &["budget"]);
    let legal = tag(&mut b, &["legal"]);
    let supplies = tag(&mut b, &["supplies"]);

    // Work / Planning
    let q4_task = b.task(T {
        priority: Priority::Normal,
        due: Some("2026-11-21"),
        reminder: Some("2026-11-21T17:00:00Z"),
        my_day: true,
        tags: q4.clone(),
        steps: &[
            "Collect shipped features from the changelog",
            "Draft three themes for next quarter",
            "Estimate effort per theme",
            "Review the draft with Alex",
            "Publish the final deck to the team wiki",
        ],
        steps_done: 2,
        notes: "Keep it to one page. Compare against the Q3 goals and flag anything that slipped, with a one-line reason each.",
        repeat: Some(Weekday::Sat),
        edited_by: Some(("codex", "2026-11-21T09:12:00Z")),
        mail: Some((
            "Q4 roadmap: pre-read and agenda",
            "dana@example.com",
            "2026-11-20T16:48:00Z",
        )),
        // A document opens; anything else, like this installer, can only be saved.
        files: &[
            ("Q4 agenda.pdf", "application/pdf", 248_000),
            ("roadmap-tool-setup.exe", "application/octet-stream", 3_400_000),
        ],
        ..t(planning, "Prepare Q4 roadmap review")
    });
    b.task(T {
        my_day: true,
        done: Some("2026-11-21T09:05:00Z"),
        ..t(planning, "Write the stand-up notes")
    });
    b.task(T {
        priority: Priority::Low,
        due: Some("2026-11-26"),
        tags: docs.clone(),
        steps: &[
            "List the accounts",
            "Update the screenshots",
            "Ask HR to review",
        ],
        steps_done: 1,
        ..t(planning, "Update the onboarding checklist")
    });
    // Work / Finance
    b.task(T {
        priority: Priority::High,
        due: Some("2026-11-19"),
        my_day: true,
        tags: tax.clone(),
        ..t(finance, "Pay quarterly VAT filing")
    });
    b.task(T {
        priority: Priority::Normal,
        due: Some("2026-11-25"),
        tags: budget.clone(),
        steps: &[
            "Pull the actuals",
            "Compare with plan",
            "Write the summary",
            "Send to finance",
        ],
        by: Some("claude"),
        created: "2026-11-21T08:02:00Z",
        ..t(finance, "Draft Q1 budget summary")
    });
    b.task(T {
        done: Some("2026-11-21T08:40:00Z"),
        ..t(finance, "Approve the travel expenses")
    });
    // Work / Office
    b.task(T {
        priority: Priority::High,
        due: Some("2026-11-27"),
        tags: legal.clone(),
        edited_by: Some(("codex", "2026-11-21T09:30:00Z")),
        ..t(office, "Review the vendor contract")
    });
    b.task(T {
        tags: supplies.clone(),
        by: Some("pi"),
        created: "2026-11-19T15:00:00Z",
        ..t(office, "Order printer toner")
    });
    b.task(t(office, "Archive the old design files"));
    b.task(T {
        done: Some("2026-11-21T10:10:00Z"),
        ..t(office, "Book the meeting room for Monday")
    });
    // Work / Events
    b.task(T {
        due: Some("2026-11-21"),
        reminder: Some("2026-11-21T12:00:00Z"),
        my_day: true,
        mail: Some((
            "Venue booking for 4 Dec",
            "events@example.com",
            "2026-11-20T11:20:00Z",
        )),
        ..t(events, "Send venue booking confirmation")
    });
    b.task(T {
        due: Some("2026-11-24"),
        tags: retro.clone(),
        ..t(events, "Book a room for the retro")
    });
    b.task(T {
        due: Some("2026-11-21"),
        reminder: Some("2026-11-21T11:55:00Z"),
        tags: retro.clone(),
        by: Some("claude"),
        created: "2026-11-21T07:40:00Z",
        ..t(events, "Confirm catering numbers for the retro")
    });
    b.task(T {
        due: Some("2026-11-22"),
        reminder: Some("2026-11-22T15:00:00Z"),
        ..t(events, "Call Alex about the venue")
    });
    // Projects
    b.task(T {
        priority: Priority::High,
        due: Some("2026-11-21"),
        my_day: true,
        tags: review.clone(),
        by: Some("claude"),
        created: "2026-11-21T08:02:00Z",
        ..t(todo_app, "Review sync-engine pull request")
    });
    b.task(T {
        due: Some("2026-11-23"),
        my_day: true,
        tags: docs.clone(),
        steps: &["Outline the changes", "Note the rollback", "Link the PR"],
        steps_done: 1,
        edited_by: Some(("codex", "2026-11-21T07:15:00Z")),
        ..t(todo_app, "Write schema migration notes")
    });
    b.task(t(todo_app, "Try the new icon set in the sidebar"));
    b.task(t(home, "Pick tiles for the bathroom"));
    b.task(T {
        due: Some("2026-12-02"),
        ..t(home, "Get a quote for the windows")
    });
    b.task(t(home, "Measure the hallway"));
    b.task(T {
        done: Some("2026-11-12T19:00:00Z"),
        ..t(reading, "The Design of Everyday Things")
    });
    b.task(T {
        done: Some("2026-11-03T21:00:00Z"),
        ..t(reading, "Thinking in Systems")
    });
    // Personal
    b.task(T {
        due: Some("2026-11-21"),
        reminder: Some("2026-11-21T18:30:00Z"),
        my_day: true,
        tags: domacnost.clone(),
        ..t(errands, "Koupit rohlíky a mléko")
    });
    b.task(T {
        my_day: true,
        done: Some("2026-11-21T08:15:00Z"),
        ..t(errands, "Water the plants")
    });
    b.task(T {
        my_day: true,
        done: Some("2026-11-21T09:50:00Z"),
        ..t(errands, "Reply to the landlord")
    });
    b.task(t(errands, "Pick up the dry cleaning"));
    b.task(T {
        priority: Priority::Low,
        due: Some("2026-11-23"),
        my_day: true,
        tags: pojisteni.clone(),
        by: Some("pi"),
        created: "2026-11-18T10:00:00Z",
        ..t(
            health,
            "Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení od lékaře",
        )
    });
    b.task(T {
        my_day: true,
        ..t(
            health,
            "Call the dentist to reschedule the check-up and ask about the bill from March",
        )
    });
    let passport = b.task(T {
        priority: Priority::Low,
        due: Some("2026-11-22"),
        reminder: Some("2026-11-22T09:00:00Z"),
        my_day: true,
        ..t(travel, "Renew passport appointment")
    });
    b.task(T {
        due: Some("2026-11-27"),
        ..t(travel, "Pack the weekend bag")
    });

    // A backlog of finished work, spread over the autumn.
    const DONE: [&str; 12] = [
        "Send the meeting notes",
        "Renew the domain",
        "File the receipts",
        "Fix the squeaky door",
        "Return the library books",
        "Update the team calendar",
        "Back up the photos",
        "Order new filters",
        "Plan the offsite agenda",
        "Pay the electricity bill",
        "Clean out the inbox",
        "Check the tyre pressure",
    ];
    let homes = [
        planning, finance, office, events, todo_app, home, errands, health, travel,
    ];
    for i in 0..96usize {
        let list = homes[i % homes.len()];
        let day = 1 + (i % 19);
        let id = b.task(T {
            created: "2026-10-01T08:00:00Z",
            ..t(list, DONE[i % DONE.len()])
        });
        b.run(
            &owner(),
            &format!("2026-11-{day:02}T16:00:00Z"),
            Command::CompleteTask { id },
        );
    }

    let lists = Lists {
        planning,
        finance,
        office,
        events,
        todo_app,
        home,
        reading,
        errands,
        health,
        travel,
        passport,
        q4: q4_task,
    };
    (b, lists)
}

fn online(at: &str) -> SyncInfo {
    SyncInfo {
        paired: true,
        connection: todo_client::Connection::Online,
        pending: 0,
        failed: 0,
        unauthorized: false,
        synced_at: Some(ts(at)),
    }
}

fn fixture(b: Builder, ui: UiState, now: &str) -> Fixture {
    if !b.errors.is_empty() {
        tracing::error!("fixture data has errors: {:?}", b.errors);
    }
    Fixture {
        state: b.state,
        ui,
        sync: online("2026-11-21T10:34:00Z"),
        now: Now::fixed(now),
        quick_add_text: None,
    }
}

/// My Day, 3 of 12 done (the F2 light mockup).
pub fn my_day() -> Fixture {
    let (b, l) = sample();
    fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    )
}

/// The Work collection, with quick-add typing (the F2 dark mockup).
pub fn work() -> Fixture {
    let (b, l) = sample();
    let work = b.state.lists[&l.planning].collection;
    let mut f = fixture(
        b,
        UiState {
            room: work.map_or(RoomKey::default(), RoomKey::Collection),
            task: Some(l.q4),
            ..UiState::default()
        },
        "2026-11-21T14:10:00Z",
    );
    f.sync = online("2026-11-21T14:09:00Z");
    f.quick_add_text = Some("Draft the changelog for 0.4 tomorrow");
    f
}

/// A selected list inside the collapsed Work collection.
fn collapsed_collection() -> Fixture {
    let (b, l) = sample();
    let ui = UiState {
        room: RoomKey::List(l.planning),
        task: Some(l.q4),
        collapsed_collections: b.state.lists[&l.planning].collection.into_iter().collect(),
        ..UiState::default()
    };
    fixture(b, ui, MORNING)
}

/// More lists than the sidebar can show: the rows keep their heights and the pane scrolls.
fn long_sidebar() -> Fixture {
    let (mut b, l) = sample();
    let errands = b.collection("Side projects", "fern");
    for name in [
        "Garden plan",
        "Bike repairs",
        "Photo archive",
        "Recipes",
        "Board games",
        "Language course",
        "Workshop",
        "Travel ideas",
        "Gift ideas",
        "Car",
    ] {
        b.list(errands, name, "jade");
    }
    let ui = UiState {
        room: RoomKey::List(l.planning),
        task: Some(l.q4),
        ..UiState::default()
    };
    fixture(b, ui, MORNING)
}

/// The name of the long list in `long-names`.
pub const LONG_LIST: &str = "Planning the quarterly roadmap and every review meeting";

/// My Day with a long list name and more tags than a row has room for: the row's chip ends in an ellipsis,
/// the tags that don't fit are counted, and the detail pane's breadcrumb gives way to its actions.
fn long_names() -> Fixture {
    let (mut b, l) = sample();
    b.run(
        &owner(),
        "2026-01-11T09:00:00Z",
        Command::UpdateList {
            id: l.planning,
            name: Some(LONG_LIST.into()),
            color: None,
        },
    );
    for name in ["roadmap", "leadership", "quarterly-review"] {
        let tag = b.tag(name);
        b.run(
            &owner(),
            "2026-11-21T09:20:00Z",
            Command::AddTag { task: l.q4, tag },
        );
    }
    fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    )
}

/// A list with nothing open: scenery only, no sun.
pub fn empty() -> Fixture {
    let (b, l) = sample();
    fixture(
        b,
        UiState {
            room: RoomKey::List(l.reading),
            ..UiState::default()
        },
        MORNING,
    )
}

/// My Day at the end of the day: every task done, the dusk scene, and tomorrow's plan.
pub fn finished() -> Fixture {
    let (mut b, l) = sample();
    // The passport appointment is tomorrow's, so it leaves today's plan; everything else gets done.
    b.run(
        &owner(),
        "2026-11-21T12:00:00Z",
        Command::SetMyDay {
            id: l.passport,
            date: None,
        },
    );
    let today = date("2026-11-21");
    let open: Vec<TaskId> = b
        .state
        .tasks
        .values()
        .filter(|t| t.my_day == Some(today) && !t.is_done())
        .map(|t| t.id)
        .collect();
    for (i, id) in open.into_iter().enumerate() {
        let at = format!("2026-11-21T{:02}:20:00Z", 11 + i);
        b.run(&owner(), &at, Command::CompleteTask { id });
    }
    let mut f = fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            ..UiState::default()
        },
        "2026-11-21T21:40:00Z",
    );
    f.sync = online("2026-11-21T21:39:00Z");
    f
}

/// Before pairing: the sample data isn't shown; the pairing view draws its own screen.
pub fn pairing() -> Fixture {
    Fixture {
        state: State::new(),
        ui: UiState::default(),
        sync: SyncInfo::signed_out(),
        now: Now::fixed(MORNING),
        quick_add_text: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Due;

    #[test]
    fn the_sample_data_builds_without_domain_errors() {
        let (b, _) = sample();
        assert!(b.errors.is_empty(), "{:#?}", b.errors);
        let (mut b, l) = sample();
        b.run(
            &owner(),
            MORNING,
            Command::SetMyDay {
                id: l.passport,
                date: None,
            },
        );
        assert!(b.errors.is_empty(), "{:#?}", b.errors);
    }

    #[test]
    fn my_day_matches_the_mockup() {
        let shell = my_day().shell();
        assert_eq!(shell.room.summary(), (3, 12, 9));
        assert_eq!(shell.room.tasks.len(), 9);
        assert_eq!(shell.room.completed_count, 3);
        let my_day = &shell.sidebar.smart[0];
        assert_eq!(my_day.count, Some(9));
        assert_eq!(shell.sidebar.collections.len(), 3);
        let selected = shell
            .selected_task
            .and_then(|i| shell.row(i))
            .expect("selected");
        assert_eq!(selected.title.as_ref(), "Prepare Q4 roadmap review");
        assert_eq!(selected.steps, Some((2, 5)));
        assert_eq!(selected.due, Due::Plain("Today 17:00".into()));
        let detail = shell.detail.expect("detail");
        assert_eq!(detail.steps.len(), 5);
        assert!(
            detail.subtitle.contains("Edited today 09:12"),
            "{}",
            detail.subtitle
        );
        assert_eq!(detail.links.len(), 1);
        let venue = shell
            .room
            .tasks
            .iter()
            .find(|t| t.title.starts_with("Send venue"))
            .expect("venue");
        assert_eq!(venue.due, Due::Soon("in 1h 25m".into()));
        assert!(venue.from_mail);
        let vat = shell
            .room
            .tasks
            .iter()
            .find(|t| t.title.starts_with("Pay"))
            .expect("vat");
        assert_eq!(vat.due, Due::Overdue("2d overdue".into()));
        assert_eq!(vat.priority, 3);
        assert!(shell.room.tasks.iter().any(|t| t.title.contains('ř')));
    }

    #[test]
    fn every_fixture_exists_and_keeps_its_numbers_consistent() {
        for name in crate::args::FIXTURES {
            let f = by_name(name).unwrap_or_else(|| panic!("{name}"));
            let shell = f.shell();
            let (done, total, open) = shell.room.summary();
            assert!(done <= total);
            assert_eq!(done + open, total);
            if let Some(i) = shell.selected_task {
                assert!(i < shell.row_count());
            }
        }
        assert!(by_name("inbox").is_none());
    }

    #[test]
    fn long_names_has_a_long_list_and_more_tags_than_fit() {
        let shell = long_names().shell();
        let q4 = shell
            .selected_task
            .and_then(|i| shell.row(i))
            .expect("selected");
        assert_eq!(q4.list.name.as_ref(), LONG_LIST);
        assert_eq!(q4.tags.len(), 5);
        assert!(q4.from_mail && q4.steps.is_some());
        let detail = shell.detail.expect("detail");
        assert_eq!(detail.list.name.as_ref(), LONG_LIST);
    }

    #[test]
    fn the_finished_day_hands_over_to_tomorrow() {
        let shell = finished().shell();
        assert!(shell.room.finished_day());
        let next = shell.room.tomorrow.expect("tomorrow");
        assert_eq!(next.weekday.as_ref(), "Sunday");
        assert_eq!(next.first.title.as_ref(), "Renew passport appointment");
        assert_eq!(next.first_when.as_ref(), "Sun 22 Nov, 09:00");
        assert_eq!(next.also.len(), 1);
        assert_eq!(next.week.len(), 3);
        assert_eq!(shell.sidebar.smart[0].count, Some(0));
    }

    #[test]
    fn the_empty_list_counts_what_was_finished() {
        let shell = empty().shell();
        assert!(shell.room.tasks.is_empty());
        assert_eq!(shell.room.summary(), (0, 0, 0));
        assert_eq!(shell.room.finished_this_month, 2);
        let reading = shell
            .sidebar
            .collections
            .iter()
            .flat_map(|c| c.lists.iter())
            .find(|l| l.list.name.as_ref() == "Reading list")
            .expect("reading list");
        assert_eq!(reading.count, 0);
    }
}
