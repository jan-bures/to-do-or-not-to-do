//! The `--screenshot` fixtures: invented sample data, never real tasks.
//!
//! Each fixture is a `todo_core::State` built by running commands through `decide` and `apply` (as agents
//! and the owner, at fixed times), plus the window's UI state, a fake sync status and a fixed clock. The
//! screenshot renders it through the same derivation (`model::derive`) as the running app, so the gallery
//! shows the real views, not a mock-up of them. No server is involved.

use todo_core::command::{Command, ListFeaturesPatch, NewTask, Placement, TaskPatch};
use todo_core::ids::{
    AttachmentId, CollectionId, LinkId, ListId, ReminderId, StepId, TagId, TaskId, uuid_v7,
};
use todo_core::model::{EmailLink, LinkKind, ListView, Priority, WorkflowStatus};
use todo_core::recurrence::{Recurrence, Rule};
use todo_core::time::{Date, Timestamp, Weekday};
use todo_core::{AccessLevel, Actor, AgentId, Grants, State, apply, decide};

use crate::model::layout::Pane;
use crate::model::{Now, RoomKey, Smart, SyncInfo, UiState};
use crate::theme::{desktop, motion as m};

/// A fixture: data, UI state, sync status and clock.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub state: State,
    pub ui: UiState,
    pub sync: SyncInfo,
    pub now: Now,
    /// Text typed into the room's quick-add, to show its focused state.
    pub quick_add_text: Option<&'static str>,
    /// A moment part way through a transition, which the screenshot freezes (`ShellView::freeze`).
    pub moment: Option<Moment>,
    /// Changes the server refused ("couldn't sync").
    pub failed: Vec<todo_client::FailedEntry>,
    /// The activity feed, newest first (the Activity room reads it a page at a time).
    pub activity: Vec<todo_client::activity::ActivityItem>,
}

/// A moment part way through a transition, for the gallery's frozen frames. Times are milliseconds into
/// the transition, made from the design's durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// `task` was just completed. With `end`, the list is scrolled to its end first.
    Completing {
        task: TaskId,
        at: u32,
        end: bool,
    },
    /// `task`, completed at `completed`, was just reopened.
    Reopening {
        task: TaskId,
        completed: Timestamp,
        at: u32,
    },
    /// The sun travelling from `from` (done, total) to the fixture's count.
    SunTravel {
        from: (u32, u32),
        at: u32,
    },
    /// The completion toast for this task; its text uses the actual task title.
    UndoToast {
        task: TaskId,
        at: u32,
    },
    /// Dusk falling over the finished day.
    Dusk {
        at: u32,
    },
    RoomSwitch {
        from: RoomKey,
        at: u32,
    },
    DetailOpen {
        at: u32,
    },
    CollectionCollapse {
        at: u32,
    },
    ScrollbarFade {
        at: u32,
    },
    DetailDusk {
        task: TaskId,
        at: u32,
    },
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
        "my-day" | "quick-add" | "caption-hover" | "caption-close-hover" => Some(my_day()),
        "ai-empty" | "ai-proposal" | "ai-batch" | "ai-settings" | "ai-error" | "ai-blocked" => {
            let mut f = my_day();
            f.ui.task = None;
            if let Some(task) = f
                .state
                .tasks
                .values_mut()
                .find(|task| task.title == "Koupit rohlíky a mléko")
            {
                task.updated.by = agent(todo_core::actor::ASSISTANT_AGENT_ID);
            }
            Some(f)
        }
        "multi-select" => {
            let mut f = my_day();
            f.ui.task = None;
            Some(f)
        }
        "work" => Some(work()),
        "list-status" | "list-settings" | "board" | "board-detail" | "timeline"
        | "timeline-month" | "timeline-tray" | "timeline-planned" => Some(workflow_room(name)),
        "quick-add-chips" | "quick-add-calendar" | "quick-add-priority" => Some(Fixture {
            quick_add_text: Some("Water the plants zítra !!"),
            ..my_day()
        }),
        "drag-reorder" | "drag-destination" => {
            let (mut b, l) = sample();
            b.task(t(l.planning, "Sketch the workshop agenda"));
            Some(fixture(
                b,
                UiState {
                    room: RoomKey::List(l.planning),
                    ..UiState::default()
                },
                MORNING,
            ))
        }
        "popout-pinned" | "popout-unpinned" => Some(popout()),
        // Keep the same detail and task row in both captures, outside the My Day filter.
        "toggle-states-on" | "toggle-states-off" => {
            let mut f = my_day();
            f.ui.room = RoomKey::Smart(Smart::All);
            let task = f.state.tasks.get_mut(&f.ui.task?)?;
            let on = name == "toggle-states-on";
            task.my_day = on.then(|| f.now.today());
            task.priority = if on { Priority::High } else { Priority::None };
            Some(f)
        }
        // The menus, open at rest: the room's "more" menu in a list, the account menu and the detail
        // pane's priority picker (`ShellView::preview_menu` opens them).
        "room-menu" => Some(list_room()),
        "account-menu"
        | "device-settings"
        | "device-invite"
        | "device-approve"
        | "updates-disabled"
        | "updates-up-to-date"
        | "updates-checking"
        | "updates-downloading"
        | "updates-ready"
        | "updates-indicator"
        | "updates-failed"
        | "priority-menu"
        | "detail-due-calendar"
        | "repeat-focus"
        | "delete-tag-confirm" => Some(my_day()),
        // The reminder picker on the open task's reminder (Today 17:00): its presets, and its custom
        // page with a time typed; the row with the keyboard on it.
        "reminder-menu" | "reminder-custom" | "reminder-focus" => Some(my_day()),
        // The row of a task without a reminder, at rest.
        "reminder-none" => {
            let mut f = my_day();
            let task = f.ui.task?;
            f.state.reminders.retain(|_, r| r.task != task);
            Some(f)
        }
        "repeat-menu" | "repeat-none" | "repeat-custom" | "repeat-weekly" | "repeat-monthly" => {
            let mut f = my_day();
            let t = f.state.tasks.get_mut(&f.ui.task?)?;
            t.recurrence = match name {
                "repeat-none" | "repeat-menu" => {
                    t.due = None;
                    None
                }
                "repeat-monthly" => Some(Recurrence::new(Rule::Monthly {
                    interval: 1,
                    day: 21,
                })),
                _ => Some(Recurrence::new(Rule::Weekly {
                    interval: 2,
                    days: [Weekday::Mon, Weekday::Thu].into(),
                })),
            };
            Some(f)
        }
        // A task's context menu, opened on a row other than the selected one; the list picker that
        // moves the selected task (its menu's Move to…, or M), grouped by collection.
        "task-context-menu" | "move-to-picker" | "merge-task-picker" | "merge-task-confirm" => {
            Some(my_day())
        }
        // The sidebar's "3 couldn't sync", opened: each refused change in plain words, with Dismiss.
        "sync-failures" => {
            let mut f = my_day();
            f.failed = failed_changes(&f.state);
            Some(f)
        }
        // Deleting a list with a long name, confirmed from its room's menu: the whole name and the
        // count wrap above the buttons.
        "delete-list-confirm" => Some(long_list_room()),
        // Organizing: a new collection being named in place; a collection's and a list's context
        // menus at their sidebar rows; deleting the Work collection from its room's menu; a list's
        // room menu turned into the collection picker and the colour swatches.
        "new-collection" => Some(new_collection()),
        // Tab onto the pinned New button without scrolling the rooms.
        "tab-new" | "tab-new-collection" | "sidebar-footer" | "sidebar-new-menu" => {
            Some(long_sidebar())
        }
        "sidebar-footer-min" => {
            let mut f = long_sidebar();
            f.ui.panes = f.ui.panes.with(Pane::Sidebar, desktop::SIDEBAR_MIN);
            f.sync.connection = todo_client::Connection::Offline;
            f.sync.pending = 3;
            Some(f)
        }
        "collection-menu" | "list-context-menu" => Some(my_day()),
        "delete-collection-confirm" => {
            let mut f = work();
            f.quick_add_text = None;
            Some(f)
        }
        "move-list-picker" | "list-colour" => Some(list_room()),
        // Tags: the picker on the detail pane's Tags row, with a new name typed, a tag's own page, its
        // colour swatches, and renaming it in the row.
        "tag-picker" | "tag-create" | "tag-edit" | "tag-colour" | "tag-rename" => Some(my_day()),
        // Links: the task's Links… page, and a link being typed into the detail pane's field.
        "task-links" | "add-link" => Some(with_web_link()),
        "task-activity" => Some(task_activity()),
        "attachments-list" => Some(my_day()),
        "attachments-empty" | "attachments-uploading" | "attachments-error" => {
            let mut f = my_day();
            let task = f.ui.task?;
            f.state.attachments.retain(|_, a| a.task != task);
            Some(f)
        }
        "collapsed-collection" => Some(collapsed_collection()),
        "collection-collapse-50" => Some(Fixture {
            moment: Some(Moment::CollectionCollapse { at: m::PANEL / 2 }),
            ..collapsed_collection()
        }),
        "scrollbar-fade-50" => Some(Fixture {
            moment: Some(Moment::ScrollbarFade {
                at: m::SCROLLBAR_HOLD + m::SCROLLBAR_OUT / 2,
            }),
            ..long_sidebar()
        }),
        "long-sidebar" => Some(long_sidebar()),
        // The smallest window and narrowest sidebar, also when editing a new list or collection.
        "short-sidebar" | "short-sidebar-new-list" | "short-sidebar-new-collection" => {
            let mut f = long_sidebar();
            f.ui.panes = f.ui.panes.with(Pane::Sidebar, desktop::SIDEBAR_MIN);
            Some(f)
        }
        "sidebar-wide" => {
            let mut f = long_sidebar();
            f.ui.panes = f.ui.panes.with(Pane::Sidebar, desktop::SIDEBAR_MAX);
            Some(f)
        }
        "detail-narrow" => {
            let mut f = long_names();
            f.ui.panes = f.ui.panes.with(Pane::Detail, desktop::DETAIL_MIN);
            Some(f)
        }
        "detail-wide" | "detail-wide-1920" => Some(wide_detail()),
        "resize-handle-hover" | "email-drop" => Some(my_day()),
        "long-names" | "narrow-detail" => Some(long_names()),
        "long-names-narrow" => Some(long_names_narrow()),
        "long-task-list" => Some(long_task_list()),
        "empty" => Some(empty()),
        "finished" | "finished-short" => Some(finished()),
        "finished-nothing-tomorrow" => Some(finished_nothing_tomorrow()),
        // Tomorrow's first task completed from the finished day's hand-over, half way through its
        // collapse: dimmed, struck through, fading and closing its height.
        "handover-complete-50" => Some(handover_complete(
            m::CHECK_TICK_DELAY + m::CHECK_TICK + m::COMPLETE_HOLD + m::ROW_COLLAPSE / 2,
        )),
        "pairing" | "pairing-code" | "open-failed" | "open-failed-confirm" => Some(pairing()),
        // Activity: the feed; one agent's changes; nothing yet; an undo waiting to be sent; and an
        // undo the server refused, in plain words beside its change (`ShellView::preview_menu` asks
        // for those two undos, as Enter on the selected change would).
        "activity" | "activity-pending" | "activity-refused" | "activity-menu" => {
            Some(activity(None))
        }
        "activity-agent" => Some(activity(Some("claude"))),
        "trash" | "trash-empty" => Some(trash(name == "trash-empty")),
        "activity-empty" => {
            let mut f = activity(None);
            f.activity.clear();
            Some(f)
        }
        // The completion choreography, frozen: the disc popping and the tick drawing, the hold, half way
        // through the collapse; the sun half way to the new count; a reopened task half way out of
        // Completed.
        "completing-fill" => Some(completing(m::CHECK_TICK_DELAY + m::CHECK_TICK / 4)),
        "completing-hold" => Some(completing(
            m::CHECK_TICK_DELAY + m::CHECK_TICK + m::COMPLETE_HOLD / 2,
        )),
        "completing-collapse-50" => Some(completing(
            m::CHECK_TICK_DELAY + m::CHECK_TICK + m::COMPLETE_HOLD + m::ROW_COLLAPSE / 2,
        )),
        "sun-travel-50" => Some(sun_travel(m::SUN_TRAVEL / 2)),
        "undo-toast" | "narrow-undo-toast" => Some(undo_toast(m::UNDO_WINDOW / 2)),
        "toast-in-50" => Some(undo_toast(m::TOAST_IN / 2)),
        "reopening-collapse-50" => Some(reopening(m::OVERLAY_OUT + m::ROW_COLLAPSE / 2)),
        // A list longer than the room, part way through a collapse: at the top (the row from below the
        // fold rises into view), and scrolled to its end (the rows above come down instead).
        "long-list-collapse-80" => Some(long_list_collapse("Check the room capacity", 80, false)),
        "long-list-end-collapse-50" => Some(long_list_collapse("Prepare reusable signs", 50, true)),
        // Dusk falling: half way (the ink has switched, the night is coming), and three quarters of the
        // way (tomorrow's hand-over entering, staggered).
        "dusk-50" => Some(dusk(m::DUSK / 2)),
        "dusk-75" => Some(dusk(m::DUSK * 3 / 4)),
        "dusk-one-task-50" => Some(dusk_one_task(m::SUN_TRAVEL / 2)),
        "room-switch-50" => Some(room_switch(m::CONTENT_SWAP / 2)),
        "room-hue-50" => Some(room_switch(m::ROOM_HUE / 2)),
        "detail-open-50" => Some(Fixture {
            moment: Some(Moment::DetailOpen { at: m::PANEL / 2 }),
            ..my_day()
        }),
        "detail-dusk-50" => {
            let mut f = finished();
            let task = f
                .state
                .tasks
                .values()
                .find(|t| t.my_day == Some(f.now.today()) && t.is_done())?
                .id;
            f.moment = Some(Moment::DetailDusk {
                task,
                at: m::PANEL / 2,
            });
            Some(f)
        }
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
        new.start_date = t.start.map(date);
        new.status = t.status;
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
    start: Option<&'static str>,
    status: WorkflowStatus,
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
        start: None,
        status: WorkflowStatus::Todo,
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
        "Check the tire pressure",
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
        moment: None,
        failed: Vec::new(),
        activity: activity_items(),
    }
}

fn workflow_room(name: &str) -> Fixture {
    let timeline = name.starts_with("timeline");
    let mut b = Builder::new();
    let home = b.collection("Home", "fern");
    let renovation = b.list(home, "Home renovation", "clay");
    let groceries = b.list(home, "Groceries", "teal");
    let work = b.collection("Work", "lagoon");
    let launch = b.list(work, "Q4 Launch", "azure");
    let admin = b.list(work, "Admin", "cornflower");
    b.run(
        &owner(),
        MORNING,
        Command::UpdateList {
            id: renovation,
            name: None,
            color: None,
            features: Some(ListFeaturesPatch {
                workflow: Some(true),
                timeline: Some(true),
                default_view: Some(if name == "list-status" {
                    ListView::List
                } else if timeline {
                    ListView::Timeline
                } else {
                    ListView::Board
                }),
            }),
        },
    );
    let hallway = b.tag("hallway");
    let bathroom = b.tag("bathroom");
    let permits = b.tag("permits");
    let lights = b.tag("lighting");
    let study = b.tag("study");
    let carpentry = b.tag("carpentry");
    let materials = b.tag("materials");
    let selected = b.task(T {
        start: Some("2026-11-19"), due: Some("2026-11-23"), priority: Priority::High, my_day: true,
        status: WorkflowStatus::InProgress, tags: vec![hallway], steps_done: 2,
        steps: &["Remove coat rack and switch plates", "Steam and peel old hallway paper", "Fill plaster cracks and sand smooth", "Roll two coats of matte primer"],
        notes: "Use low-VOC primer from the utility cupboard. Keep door trim taped until painting starts Tuesday.",
        reminder: Some("2026-11-23T08:30:00Z"), created: "2026-11-15T08:00:00Z",
        ..t(renovation, "Strip wallpaper and prime hallway walls")
    });
    b.task(T {
        due: Some("2026-11-20"),
        my_day: true,
        status: WorkflowStatus::InProgress,
        tags: vec![bathroom],
        edited_by: Some(("codex", "2026-11-21T09:37:00Z")),
        ..t(renovation, "Pick grout color for bathroom tiles")
    });
    b.task(T {
        start: Some("2026-11-20"),
        due: Some("2026-11-26"),
        status: WorkflowStatus::InProgress,
        tags: vec![permits],
        edited_by: Some(("claude", "2026-11-21T09:42:00Z")),
        ..t(
            renovation,
            "Vyřídit ohlášení drobných stavebních úprav na úřadě",
        )
    });
    b.task(T {
        start: Some("2026-11-23"),
        due: Some("2026-11-25"),
        priority: Priority::Normal,
        tags: vec![materials],
        steps: &["Request oak", "Request walnut", "Compare finishes"],
        steps_done: 1,
        edited_by: Some(("claude", "2026-11-21T09:44:00Z")),
        ..t(renovation, "Order matte oak flooring samples")
    });
    b.task(T {
        due: Some("2026-11-24"),
        priority: Priority::High,
        my_day: true,
        mail: Some((
            "Kitchen outlet visit",
            "you@example.com",
            "2026-11-17T10:00:00Z",
        )),
        ..t(renovation, "Confirm electrician visit for kitchen outlets")
    });
    b.task(T {
        start: Some("2026-11-26"),
        due: Some("2026-11-27"),
        tags: vec![bathroom],
        steps: &["Measure mirror", "Measure alcove"],
        ..t(renovation, "Measure bathroom vanity niche and mirror")
    });
    b.task(T {
        status: WorkflowStatus::Backlog,
        tags: vec![lights],
        by: Some("pi"),
        ..t(renovation, "Compare kitchen pendant lights")
    });
    b.task(T {
        start: Some(if timeline { "2026-11-30" } else { "2026-12-03" }),
        due: Some(if timeline { "2026-12-02" } else { "2026-12-07" }),
        status: WorkflowStatus::Backlog,
        tags: vec![carpentry],
        ..t(
            renovation,
            if timeline {
                "Get hallway wardrobe quote"
            } else {
                "Get quote for hallway built-in wardrobe"
            },
        )
    });
    if timeline {
        b.task(T {
            status: WorkflowStatus::Backlog,
            tags: vec![study],
            ..t(renovation, "Research acoustic panels for study")
        });
    }
    if name == "timeline-planned" {
        b.task(T {
            start: Some("2026-11-22"),
            due: Some("2026-11-28"),
            ..t(launch, "Prepare release screenshots")
        });
        b.task(T {
            start: Some("2026-11-30"),
            ..t(launch, "Draft the announcement")
        });
        b.task(T {
            due: Some("2026-11-25"),
            ..t(admin, "Review the budget")
        });
        b.task(T {
            due: Some("2026-11-26"),
            ..t(groceries, "Pick up pantry staples")
        });
    }
    for (title, done) in [
        ("Deliver old sofa to charity depot", "2026-11-21T09:15:00Z"),
        (
            "Shut off radiator valve in guest room",
            "2026-11-21T08:40:00Z",
        ),
        ("Send floor plan PDF to contractor", "2026-11-20T10:00:00Z"),
    ] {
        b.task(T {
            due: timeline.then_some("2026-11-20"),
            done: Some(done),
            ..t(renovation, title)
        });
    }
    for title in [
        "Sketch the kitchen",
        "Check the floor plan",
        "Measure window trim",
        "Catalog paint samples",
        "Map electrical outlets",
        "Label storage boxes",
        "Measure door frames",
        "Clean the entryway",
        "Book a delivery slot",
        "Sort spare tiles",
        "Find the tool manual",
    ] {
        b.task(T {
            created: "2026-11-01T08:00:00Z",
            done: Some("2026-11-12T09:00:00Z"),
            ..t(renovation, title)
        });
    }
    for title in ["Oats", "Lemons", "Chickpeas", "Yogurt", "Flour", "Coffee"] {
        b.task(t(groceries, title));
    }
    for title in [
        "Write the launch notes",
        "Review the checklist",
        "Prepare screenshots",
        "Draft the announcement",
    ] {
        b.task(t(launch, title));
    }
    for title in ["Archive the invoices", "Renew the library card"] {
        b.task(t(admin, title));
    }
    let mut f = fixture(
        b,
        UiState {
            room: if name == "timeline-planned" {
                RoomKey::Smart(Smart::Planned)
            } else {
                RoomKey::List(renovation)
            },
            task: matches!(name, "list-status" | "board-detail").then_some(selected),
            timeline: crate::model::timeline::Preferences {
                zoom: if name == "timeline-month" {
                    todo_client::timeline::Zoom::Month
                } else {
                    todo_client::timeline::Zoom::TwoWeeks
                },
                tray: name == "timeline-tray",
                ..Default::default()
            },
            ..UiState::default()
        },
        MORNING,
    );
    if name == "list-status" {
        f.quick_add_text = Some("Install hallway sconces");
    }
    f
}

fn task_activity() -> Fixture {
    let mut b = Builder::new();
    let collection = b.collection("Studio", "azure");
    let list = b.list(collection, "Design", "azure");
    let selected = b.task(T {
        by: Some("codex"),
        created: "2026-11-19T09:35:00Z",
        ..t(list, "Review the color proofs")
    });
    b.run(
        &agent("pi"),
        "2026-11-21T08:05:00Z",
        Command::CompleteTask { id: selected },
    );
    b.run(
        &owner(),
        "2026-11-21T09:23:00Z",
        Command::UpdateTask {
            id: selected,
            patch: TaskPatch {
                notes: Some("The warm option is approved.".into()),
                ..TaskPatch::default()
            },
        },
    );
    fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::Completed),
            task: Some(selected),
            ..UiState::default()
        },
        "2026-11-21T09:35:00Z",
    )
}

/// Three changes the server refused, about tasks in the sample data: an edit to a task deleted
/// elsewhere, a move this device wasn't allowed to make, and a new task the server didn't accept. Each
/// carries a raw server message, which the app must never show.
pub fn failed_changes(state: &State) -> Vec<todo_client::FailedEntry> {
    use todo_client::outbox::{FailedEntry, Operation, OutboxEntry, Rejection};
    let at = ts("2026-11-21T10:20:00Z");
    let errands = state
        .lists
        .values()
        .find(|l| l.name == "Errands")
        .map(|l| l.id)
        .unwrap_or_else(|| ListId::from_uuid(uuid::Uuid::nil()));
    let new = TaskId::from_uuid(uuid_v7(1_795_000_000_000, [0xf1; 10]));
    let refused = [
        (
            Command::UpdateTask {
                id: titled(state, "Archive the old design files"),
                patch: TaskPatch {
                    title: Some("Archive the 2025 design files".into()),
                    ..TaskPatch::default()
                },
            },
            "deleted",
        ),
        (
            Command::MoveTask {
                id: titled(state, "Order printer toner"),
                list: errands,
                from_list: None,
                placement: Placement::First,
            },
            "forbidden",
        ),
        (
            Command::CreateTask(Box::new(NewTask::new(
                new,
                errands,
                "Pick up the framed poster",
            ))),
            "invalid",
        ),
    ];
    refused
        .into_iter()
        .zip(0u8..)
        .map(|((command, code), n)| FailedEntry {
            entry: OutboxEntry {
                command_id: uuid_v7(1_795_000_000_000, [0xe0 + n; 10]),
                op: Operation::Command { command },
                submitted_at: at,
                attempts: 1,
                acked: None,
                acked_epoch: None,
            },
            failed_at: at,
            rejection: Rejection {
                status: 409,
                code: code.into(),
                message: format!("raw server text {n}: entity 0192 rejected at seq 41"),
            },
        })
        .collect()
}

/// The task whose title starts with `prefix`.
fn titled(state: &State, prefix: &str) -> TaskId {
    state
        .tasks
        .values()
        .find(|t| t.title.starts_with(prefix))
        .map(|t| t.id)
        .unwrap_or_else(|| TaskId::from_uuid(uuid::Uuid::nil()))
}

/// The task the motion fixtures complete: a plain one-off task in My Day, from mail, due within the hour.
const COMPLETED: &str = "Send venue";

/// My Day with the venue task just completed, `at` ms into its choreography (still in the open rows).
fn completing(at: u32) -> Fixture {
    let (mut b, l) = sample();
    let task = titled(&b.state, COMPLETED);
    b.run(&owner(), MORNING, Command::CompleteTask { id: task });
    let mut f = fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    );
    f.ui.holding.insert(task);
    f.moment = Some(Moment::Completing {
        task,
        at,
        end: false,
    });
    f
}

fn undo_toast(at: u32) -> Fixture {
    let mut f = completing(0);
    let task = titled(&f.state, COMPLETED);
    f.ui.holding.clear();
    f.moment = Some(Moment::UndoToast { task, at });
    f
}

/// `long-task-list` with the task titled `title` `percent` of the way through its collapse, the list
/// scrolled to its end or not.
fn long_list_collapse(title: &str, percent: u32, end: bool) -> Fixture {
    let mut f = long_task_list();
    let task = titled(&f.state, title);
    let complete = Command::CompleteTask { id: task };
    match decide(&f.state, &owner(), &complete, ts(MORNING)) {
        Ok(events) => {
            for e in &events {
                if let Err(err) = apply(&mut f.state, e) {
                    tracing::error!("fixture data has errors: {err}");
                }
            }
        }
        Err(err) => tracing::error!("fixture data has errors: {err}"),
    }
    f.ui.holding.insert(task);
    f.moment = Some(Moment::Completing {
        task,
        at: m::CHECK_TICK_DELAY
            + m::CHECK_TICK
            + m::COMPLETE_HOLD
            + m::ROW_COLLAPSE * percent / 100,
        end,
    });
    f
}

/// My Day after the venue task has left: the sun `at` ms into its travel from 3 to 4 of 12.
fn sun_travel(at: u32) -> Fixture {
    let (mut b, l) = sample();
    let task = titled(&b.state, COMPLETED);
    b.run(&owner(), MORNING, Command::CompleteTask { id: task });
    let mut f = fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    );
    f.moment = Some(Moment::SunTravel { from: (3, 12), at });
    f
}

/// My Day with Completed open and its newest task just reopened, `at` ms into leaving its place there.
fn reopening(at: u32) -> Fixture {
    let (mut b, l) = sample();
    let task = titled(&b.state, "Reply to the landlord");
    let completed = b.state.tasks.get(&task).and_then(|t| t.completed_at);
    b.run(&owner(), MORNING, Command::UncompleteTask { id: task });
    let mut f = fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            task: Some(l.q4),
            show_completed: true,
            ..UiState::default()
        },
        MORNING,
    );
    if let Some(completed) = completed {
        f.ui.reopening.insert(task, completed);
        f.moment = Some(Moment::Reopening {
            task,
            completed,
            at,
        });
    }
    f
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

/// A list's room (Planning) with a task open.
pub fn list_room() -> Fixture {
    let (b, l) = sample();
    fixture(
        b,
        UiState {
            room: RoomKey::List(l.planning),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    )
}

/// Planning in its own window, including a long Czech title on the narrow row grid.
fn popout() -> Fixture {
    let (mut b, l) = sample();
    b.task(t(
        l.planning,
        "Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení",
    ));
    fixture(
        b,
        UiState {
            room: RoomKey::List(l.planning),
            task: Some(l.q4),
            compact: true,
            ..UiState::default()
        },
        MORNING,
    )
}

/// My Day with a collection just added (empty, and open), as New collection leaves it.
fn new_collection() -> Fixture {
    let (mut b, _) = sample();
    let id = b.collection("New collection", "orchid");
    fixture(
        b,
        UiState {
            room: RoomKey::Collection(id),
            task: None,
            ..UiState::default()
        },
        MORNING,
    )
}

/// My Day, the Q4 task with a web link besides its email source.
fn with_web_link() -> Fixture {
    let (mut b, l) = sample();
    let id = LinkId::from_uuid(b.uuid());
    b.run(
        &owner(),
        "2026-11-20T17:30:00Z",
        Command::AddLink {
            id,
            task: l.q4,
            kind: LinkKind::Web {
                url: "https://docs.example.com/q4/roadmap".into(),
                title: Some("Q4 roadmap draft".into()),
            },
        },
    );
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

/// A list's room (Planning, with a long name) with a task open.
fn long_list_room() -> Fixture {
    let (mut b, l) = sample();
    b.run(
        &owner(),
        "2026-01-11T09:00:00Z",
        Command::UpdateList {
            id: l.planning,
            name: Some(LONG_LIST.into()),
            color: None,
            features: None,
        },
    );
    fixture(
        b,
        UiState {
            room: RoomKey::List(l.planning),
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
    f.quick_add_text = Some("Draft the changelog for 0.4 next week");
    f
}

fn room_switch(at: u32) -> Fixture {
    let mut f = work();
    f.quick_add_text = None;
    f.ui.task = None;
    f.moment = Some(Moment::RoomSwitch {
        from: RoomKey::Smart(Smart::MyDay),
        at,
    });
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

/// The shared crowded list and tags for the name and pane-width fixtures.
fn long_names_data() -> (Builder, Lists) {
    let (mut b, l) = sample();
    b.run(
        &owner(),
        "2026-01-11T09:00:00Z",
        Command::UpdateList {
            id: l.planning,
            name: Some(LONG_LIST.into()),
            color: None,
            features: None,
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
    (b, l)
}

/// My Day with a long list name and more tags than a row has room for: the row's chip ends in an ellipsis,
/// the tags that don't fit are counted, and the detail pane's breadcrumb gives way to its actions.
fn long_names() -> Fixture {
    let (b, l) = long_names_data();
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

/// Detail at the divider's maximum, with prose long enough to show the reading measure.
fn wide_detail() -> Fixture {
    let (mut b, l) = long_names_data();
    b.run(
        &owner(),
        MORNING,
        Command::UpdateTask {
            id: l.q4,
            patch: TaskPatch {
                title: Some("Prepare the quarterly roadmap review and share the proposed milestones with the teams responsible for delivering them".into()),
                notes: Some("Keep the summary to one page so everyone can read it before the review. Compare the shipped work with the original goals and write down where the dates changed, why they changed, and what we learned. Leave room for questions that need a decision before the next planning round.".into()),
                ..TaskPatch::default()
            },
        },
    );
    // Use an existing step so the links and attachments still fit in the reference window.
    if let Some(id) = b
        .state
        .steps
        .values()
        .find(|s| s.task == l.q4 && s.completed_at.is_none())
        .map(|s| s.id)
    {
        b.run(
            &owner(),
            MORNING,
            Command::UpdateStep {
                id,
                title: "Check the proposed milestones with each project lead and record the decisions we still need before the next planning round".into(),
            },
        );
    }
    let mut f = fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    );
    f.ui.panes = f.ui.panes.with(Pane::Detail, 1920.);
    f
}

/// Enough rows and steps to overflow both the virtualized list and the detail pane.
fn long_task_list() -> Fixture {
    let (mut b, l) = sample();
    for title in [
        "Update the project brief",
        "Check the room capacity",
        "Send out the agenda",
        "Sketch the seating plan",
        "Review printing estimates",
        "Order spare name badges",
        "Confirm catering options",
        "Schedule the rehearsal",
        "Publish the program",
        "Test the room microphone",
        "Gather questions for the panel",
        "Book a captioner",
        "Prepare reusable signs",
        "Draft the follow-up note",
    ] {
        b.task(t(l.planning, title));
    }
    for i in 1..=24 {
        let id = StepId::from_uuid(b.uuid());
        b.run(
            &owner(),
            MORNING,
            Command::CreateStep {
                id,
                task: l.q4,
                title: format!("Review item {i} on the venue checklist"),
                placement: Placement::Last,
            },
        );
    }
    fixture(
        b,
        UiState {
            room: RoomKey::List(l.planning),
            task: Some(l.q4),
            ..UiState::default()
        },
        MORNING,
    )
}

/// `long-names` in the narrowest window, with nothing selected: the room keeps the window's width and its
/// title keeps its size, with the counts under it. (`narrow-detail` is `long-names` itself in that window:
/// the selected task opens over the list.)
fn long_names_narrow() -> Fixture {
    let mut f = long_names();
    f.ui.task = None;
    f
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

/// The finished day with tomorrow's first task (the hand-over's card) just completed with Space, `at` ms
/// into its choreography.
pub fn handover_complete(at: u32) -> Fixture {
    let mut f = finished();
    let shell = crate::model::derive(&f.state, &f.ui, &f.sync, &f.now);
    let Some(task) = shell.room.handover().map(|next| next.first.id) else {
        return f;
    };
    let complete = Command::CompleteTask { id: task };
    match decide(&f.state, &owner(), &complete, ts("2026-11-21T21:39:30Z")) {
        Ok(events) => {
            for e in &events {
                if let Err(err) = apply(&mut f.state, e) {
                    tracing::error!("fixture data has errors: {err}");
                }
            }
        }
        Err(err) => tracing::error!("fixture data has errors: {err}"),
    }
    f.ui.task = Some(task);
    f.ui.holding.insert(task);
    f.moment = Some(Moment::Completing {
        task,
        at,
        end: false,
    });
    f
}

/// The finished day with dusk falling, `at` ms in.
fn dusk(at: u32) -> Fixture {
    let mut f = finished();
    f.moment = Some(Moment::Dusk { at });
    f
}

/// My Day had just one task, which finished: the sun is still travelling from the far left.
fn dusk_one_task(at: u32) -> Fixture {
    let (mut b, _) = sample();
    let one = titled(&b.state, COMPLETED);
    let today = date("2026-11-21");
    let others: Vec<_> = b
        .state
        .tasks
        .values()
        .filter(|t| t.id != one && t.my_day == Some(today))
        .map(|t| t.id)
        .collect();
    for id in others {
        b.run(&owner(), MORNING, Command::SetMyDay { id, date: None });
    }
    b.run(&owner(), MORNING, Command::CompleteTask { id: one });
    let mut f = fixture(
        b,
        UiState {
            room: RoomKey::Smart(Smart::MyDay),
            ..UiState::default()
        },
        MORNING,
    );
    f.moment = Some(Moment::Dusk { at });
    f
}

/// The finished day with nothing due tomorrow: no hand-over, but planning the next day is still offered.
pub fn finished_nothing_tomorrow() -> Fixture {
    let mut f = finished();
    let tomorrow = date("2026-11-22");
    let later = date("2026-11-24");
    let due_tomorrow: Vec<TaskId> = f
        .state
        .tasks
        .values()
        .filter(|t| !t.is_done() && t.due == Some(tomorrow))
        .map(|t| t.id)
        .collect();
    let actor = owner();
    for id in due_tomorrow {
        let cmd = Command::UpdateTask {
            id,
            patch: TaskPatch {
                due: Some(Some(later)),
                ..TaskPatch::default()
            },
        };
        let at = ts("2026-11-21T21:30:00Z");
        match decide(&f.state, &actor, &cmd, at) {
            Ok(events) => {
                for e in &events {
                    if let Err(e) = apply(&mut f.state, e) {
                        tracing::error!("fixture data has errors: {e:?}");
                    }
                }
            }
            Err(e) => tracing::error!("fixture data has errors: {e:?}"),
        }
    }
    f
}

/// The sample change whose undo the fixtures leave waiting to be sent.
pub const PENDING_ACTIVITY_SEQ: u64 = 122;
/// The sample change whose undo the fixtures refuse, as the server would after a later edit.
pub const REFUSED_ACTIVITY_SEQ: u64 = 119;

/// The Activity room, every change or (`agent`) one agent's, over the sample data.
pub fn activity(agent: Option<&str>) -> Fixture {
    let mut f = my_day();
    let agent = agent
        .and_then(|a| AgentId::new(a).ok())
        .and_then(|a| crate::model::AgentKey::of(&a));
    f.ui = UiState {
        room: RoomKey::Activity(agent),
        task: None,
        ..f.ui
    };
    f
}

fn trash(empty: bool) -> Fixture {
    let mut f = my_day();
    f.ui.room = RoomKey::Trash;
    f.ui.task = None;
    if !empty {
        let (mut b, lists) = sample();
        let archived = b.collection("Spring fair", "jade");
        let flyers = b.list(archived, "Flyers", "clay");
        let draft = TaskId::from_uuid(b.uuid());
        let old = TaskId::from_uuid(b.uuid());
        b.run(
            &owner(),
            "2026-11-12T09:00:00Z",
            Command::CreateTask(Box::new(NewTask::new(
                draft,
                lists.errands,
                "Return the paint samples",
            ))),
        );
        b.run(
            &owner(),
            "2026-11-12T09:00:00Z",
            Command::CreateTask(Box::new(NewTask::new(
                old,
                flyers,
                "Send the first draft to print",
            ))),
        );
        for (id, at) in [
            (old.into(), "2026-11-17T09:00:00Z"),
            (flyers.into(), "2026-11-18T09:00:00Z"),
            (archived.into(), "2026-11-19T09:00:00Z"),
            (draft.into(), "2026-11-21T09:20:00Z"),
        ] {
            b.run(&owner(), at, Command::Delete { id });
        }
        debug_assert!(b.errors.is_empty(), "{:?}", b.errors);
        f.state = b.state;
    }
    f
}

/// Invented changes to the sample data, newest first, as the server describes them: from the owner,
/// the agents and email, over five days, more than a page of them. One was undone already.
pub fn activity_items() -> Vec<todo_client::activity::ActivityItem> {
    use todo_client::{activity::ActivityItem, protocol::LogEvent};
    use todo_core::{Event, event::EventPayload, ids::EventId};

    let you = owner();
    let mail = Actor::EmailIngest;
    let (claude, codex, pi) = (agent("claude"), agent("codex"), agent("pi"));
    let changes: [(u64, &Actor, &str, &str); 24] = [
        (
            124,
            &you,
            "2026-11-21T10:31:00Z",
            "Completed task \"Send the meeting notes\"",
        ),
        (
            123,
            &codex,
            "2026-11-21T10:12:00Z",
            "Created task \"Renew the domain\"",
        ),
        (
            PENDING_ACTIVITY_SEQ,
            &claude,
            "2026-11-21T09:58:00Z",
            "Updated task \"Venue booking for 4 Dec\" (due date)",
        ),
        (
            121,
            &mail,
            "2026-11-21T09:40:00Z",
            "Created task \"Pay the electricity bill\"",
        ),
        (
            120,
            &pi,
            "2026-11-21T09:05:00Z",
            "Added step \"Estimate effort per theme\"",
        ),
        (
            REFUSED_ACTIVITY_SEQ,
            &claude,
            "2026-11-21T08:23:00Z",
            "Updated task \"File the receipts\" (priority)",
        ),
        (
            118,
            &you,
            "2026-11-21T08:02:00Z",
            "Moved task \"Back up the photos\" to Errands",
        ),
        (
            117,
            &codex,
            "2026-11-20T17:10:00Z",
            "Created task \"Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení od lékaře\"",
        ),
        (
            116,
            &you,
            "2026-11-20T16:45:00Z",
            "Restored list \"Reading list\"",
        ),
        (
            115,
            &claude,
            "2026-11-20T15:12:00Z",
            "Updated list \"Home renovation\" (name)",
        ),
        (
            114,
            &codex,
            "2026-11-20T14:00:00Z",
            "Completed task \"Order new filters\"",
        ),
        (
            113,
            &pi,
            "2026-11-20T11:20:00Z",
            "Updated task \"Draft three themes for next quarter\" (tags)",
        ),
        (
            112,
            &mail,
            "2026-11-19T18:30:00Z",
            "Created task \"Call the dentist to reschedule the check-up and ask about the bill from March\"",
        ),
        (
            111,
            &you,
            "2026-11-19T12:00:00Z",
            "Deleted task \"Fix the squeaky door\"",
        ),
        (
            110,
            &claude,
            "2026-11-19T09:30:00Z",
            "Created task \"Plan the offsite agenda\"",
        ),
        (
            109,
            &codex,
            "2026-11-18T16:00:00Z",
            "Created task \"Update the team calendar\"",
        ),
        (
            108,
            &you,
            "2026-11-18T13:15:00Z",
            "Completed task \"Return the library books\"",
        ),
        (
            107,
            &pi,
            "2026-11-18T10:40:00Z",
            "Updated task \"Check the tire pressure\" (due date)",
        ),
        (
            106,
            &claude,
            "2026-11-17T17:05:00Z",
            "Created task \"Clean out the inbox\"",
        ),
        (105, &you, "2026-11-17T15:30:00Z", "Created list \"Travel\""),
        (
            104,
            &codex,
            "2026-11-17T11:00:00Z",
            "Completed task \"Collect shipped features from the changelog\"",
        ),
        (
            103,
            &mail,
            "2026-11-17T09:20:00Z",
            "Created task \"Venue booking for 4 Dec\"",
        ),
        (
            102,
            &you,
            "2026-11-16T19:00:00Z",
            "Updated task \"Pull the actuals\" (notes)",
        ),
        (
            101,
            &pi,
            "2026-11-16T10:10:00Z",
            "Created task \"Order printer toner\"",
        ),
    ];
    changes
        .into_iter()
        .map(|(seq, actor, at, summary)| {
            let command_id = uuid_v7(1_795_100_000_000 + seq, [0xac; 10]);
            let id = EventId::from_uuid(command_id);
            let undone = seq == 117;
            ActivityItem {
                event: LogEvent {
                    event: Event {
                        id,
                        seq: Some(seq),
                        group: id,
                        actor: actor.clone(),
                        at: ts(at),
                        undoes: None,
                        forced: false,
                        payload: EventPayload::Updated {
                            target: TaskId::from_uuid(command_id).into(),
                            changes: Vec::new(),
                        },
                    },
                    command_id,
                    command_idx: 0,
                    command_len: Some(1),
                },
                summary: summary.to_owned(),
                approved_by: None,
                undone,
                can_undo: !undone,
            }
        })
        .collect()
}

/// Before pairing: the sample data isn't shown; the pairing view draws its own screen.
pub fn pairing() -> Fixture {
    Fixture {
        state: State::new(),
        ui: UiState::default(),
        sync: SyncInfo::signed_out(),
        now: Now::fixed(MORNING),
        quick_add_text: None,
        moment: None,
        failed: Vec::new(),
        activity: activity_items(),
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
    fn detail_activity_shows_each_actors_own_change() {
        let shell = task_activity().shell();
        let detail = shell.detail.expect("selected task");
        assert_eq!(
            detail
                .activity
                .iter()
                .map(|line| (line.who.as_str(), line.action, line.when.as_str()))
                .collect::<Vec<_>>(),
            [
                ("You", "edited this", "12 min ago"),
                ("Pi", "completed this", "1 hr ago"),
                ("Codex", "created this", "2 days ago"),
            ],
        );
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
    fn wide_detail_fixtures_use_the_whole_budget_and_include_long_prose() {
        use crate::model::layout::DetailPane;

        for name in ["detail-wide", "detail-wide-1920"] {
            let f = by_name(name).expect("wide fixture");
            let (width, _) = crate::screenshot::window_size(name);
            let columns = f.ui.panes.at(width, DetailPane::Docked);
            assert_eq!(columns.room_width(), desktop::LIST_MIN_BESIDE_DETAIL);
            assert_eq!(columns.width(Pane::Detail), columns.max(Pane::Detail));
            let detail = f.shell().detail.expect("selected task");
            let measure = desktop::DETAIL_TEXT_MEASURE as usize;
            assert!(detail.title.len() > measure);
            assert!(detail.notes.len() > measure);
            assert!(detail.steps.iter().any(|s| s.title.len() > measure));
            assert!(!detail.attachments.is_empty() && !detail.links.is_empty());
        }
    }

    #[test]
    fn j_and_k_walk_the_finished_days_hand_over() {
        let mut f = finished();
        let shell = f.shell();
        let next = shell.room.handover().expect("hands over");
        let ids: Vec<TaskId> = next.tasks().map(|t| t.id).collect();
        assert_eq!(ids.len(), 5);
        assert_eq!(shell.nav_tasks(), ids);
        // With the Completed section open, its rows show and J and K walk those instead.
        f.ui.show_completed = true;
        let shell = f.shell();
        assert!(shell.room.handover().is_none());
        assert_eq!(shell.nav_tasks().len(), 11);
    }

    #[test]
    fn long_task_list_fills_the_room_and_detail_pane() {
        let shell = long_task_list().shell();
        assert!(shell.row_count() >= 14);
        assert!(shell.detail.expect("selected task").steps.len() >= 24);
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
    fn a_finished_day_with_nothing_due_tomorrow_still_offers_planning() {
        let shell = finished_nothing_tomorrow().shell();
        assert!(shell.room.finished_day());
        assert!(shell.room.tomorrow.is_none());
        assert!(shell.room.handover().is_none());
        assert_eq!(shell.room.plan_only().map(|s| s.as_ref()), Some("Sunday"));
        // Nothing to walk: no rows, no hand-over.
        assert!(shell.nav_tasks().is_empty());
        // The day with a hand-over offers its planning there instead.
        let shell = finished().shell();
        assert_eq!(shell.room.plan_weekday.as_deref(), Some("Sunday"));
        assert!(shell.room.plan_only().is_none());
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
