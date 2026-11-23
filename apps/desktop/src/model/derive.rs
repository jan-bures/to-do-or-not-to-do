//! `State` → [`Shell`].
//!
//! The room's tasks and its header (eyebrow, progress, summary, status-bar counts, empty line) come from
//! `todo_client::room_header`, which the web and Android clients share: it says what the sun measures.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use gpui::SharedString;
use todo_client::activity::{agent_counts_today, agent_status, select_agent_rows};
use todo_client::room_header;
use todo_core::State;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{LinkKind, Priority, Stamp, Task};
use todo_core::query::{EvalContext, Query, fold};
use todo_core::time::{Timestamp, add_days, days_between};

use super::labels::{self, DueLabel};
use super::lists;
use super::*;
use crate::theme::Icon;

/// Per-task facts gathered once per derivation.
struct Index {
    steps: HashMap<TaskId, (u32, u32)>,
    /// The next live reminder (the earliest from the start of today), per task.
    reminder: HashMap<TaskId, Timestamp>,
    mail: HashSet<TaskId>,
    lists: HashMap<ListId, ListRef>,
}

impl Index {
    fn new(state: &State, now: &Now) -> Self {
        let mut steps: HashMap<TaskId, (u32, u32)> = HashMap::new();
        for s in state.steps.values().filter(|s| s.deleted.is_none()) {
            let e = steps.entry(s.task).or_default();
            e.1 += 1;
            if s.completed_at.is_some() {
                e.0 += 1;
            }
        }
        let today = now.today();
        let mut reminder: HashMap<TaskId, Timestamp> = HashMap::new();
        for r in state.reminders.values().filter(|r| r.deleted.is_none()) {
            if now.date_of(r.at) < today {
                continue;
            }
            reminder
                .entry(r.task)
                .and_modify(|t| *t = (*t).min(r.at))
                .or_insert(r.at);
        }
        let mail = state
            .links
            .values()
            .filter(|l| l.deleted.is_none() && matches!(l.kind, LinkKind::Email(_)))
            .map(|l| l.task)
            .collect();
        let lists = state
            .lists
            .values()
            .map(|l| {
                let collection = l
                    .collection
                    .and_then(|c| state.collections.get(&c))
                    .map(|c| SharedString::from(labels::one_line(&c.name)));
                (
                    l.id,
                    ListRef {
                        collection,
                        name: labels::one_line(&l.name).into(),
                        hue: lists::hue(l.color.as_deref(), l.id.as_uuid().as_bytes()),
                    },
                )
            })
            .collect();
        Self {
            steps,
            reminder,
            mail,
            lists,
        }
    }

    fn list(&self, id: ListId) -> ListRef {
        self.lists.get(&id).cloned().unwrap_or(ListRef {
            collection: None,
            name: "".into(),
            hue: hues::MY_DAY,
        })
    }
}

pub(super) use todo_client::room_header::{list_live, task_live};

fn priority_marks(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
        Priority::High => 3,
    }
}

/// The agent that last touched the task, else the one that created it.
fn agent_of(task: &Task) -> Option<crate::theme::Agent> {
    lists::agent(&task.updated.by).or_else(|| lists::agent(&task.created.by))
}

fn row(state: &State, ix: &Index, task: &Task, now: &Now) -> TaskRow {
    use todo_core::query::QueryStatus;
    let features = state.lists.get(&task.list).map_or_default(|l| l.features);
    let span = features
        .timeline
        .then_some(task.start_date)
        .flatten()
        .zip(task.due)
        .map(|(start, end)| super::workflow::span(start, end, now.today()));
    let due = match labels::due(task.due, ix.reminder.get(&task.id).copied(), now) {
        DueLabel::None => Due::None,
        DueLabel::Soon(s) => Due::Soon(span.unwrap_or(s).into()),
        DueLabel::Plain(s) => Due::Plain(span.unwrap_or(s).into()),
        DueLabel::Overdue(s) => Due::Overdue(if features.workflow && s == "yesterday" {
            "1d overdue".into()
        } else {
            s.into()
        }),
    };
    TaskRow {
        id: task.id,
        title: labels::one_line(&task.title).into(),
        list: ix.list(task.list),
        tags: task
            .tags
            .iter()
            .filter_map(|t| state.tags.get(t))
            .filter(|t| t.deleted.is_none())
            .map(|t| SharedString::from(format!("#{}", labels::one_line(&t.name))))
            .collect(),
        steps: ix.steps.get(&task.id).copied(),
        agent: agent_of(task),
        priority: priority_marks(task.priority),
        due: if task.is_done() { Due::None } else { due },
        from_mail: ix.mail.contains(&task.id),
        done: task.is_done(),
        status: features.workflow.then(|| QueryStatus::of(task)),
        completed_when: task.completed_at.map(|at| labels::when(at, now).into()),
    }
}

/// The shared room key for a room.
pub(crate) fn shared_key(key: RoomKey) -> room_header::RoomKey<'static> {
    match key {
        RoomKey::Smart(s) => room_header::RoomKey::Smart(s.shared()),
        RoomKey::Collection(c) => room_header::RoomKey::Collection(c),
        RoomKey::List(l) => room_header::RoomKey::List(l),
        RoomKey::Activity(agent) => room_header::RoomKey::Activity(agent.map(AgentKey::as_str)),
        RoomKey::Trash => room_header::RoomKey::Trash,
    }
}

fn eyebrow(e: room_header::Eyebrow) -> Eyebrow {
    match e {
        room_header::Eyebrow::Day {
            weekday,
            date,
            week,
        } => Eyebrow::Day {
            weekday: weekday.into(),
            date: date.into(),
            week: week.into(),
        },
        room_header::Eyebrow::Kind { kind, detail } => Eyebrow::Kind {
            kind: kind.into(),
            detail: detail.into(),
        },
    }
}

/// The inline filter: query language when it parses, else words in the title.
fn filter<'a>(
    state: &State,
    text: &str,
    ctx: &EvalContext,
    tasks: Vec<&'a Task>,
    held_updates: &BTreeMap<TaskId, Stamp>,
) -> Vec<&'a Task> {
    let text = text.trim();
    if text.is_empty() {
        return tasks;
    }
    match Query::parse(text) {
        Ok(q) => {
            let matcher = q.matcher(state, ctx);
            tasks
                .into_iter()
                .filter(|t| {
                    room_header::matches_with_held_update(&matcher, t, held_updates.get(&t.id))
                })
                .collect()
        }
        Err(_) => {
            // Not a query: every word of it must appear in the title.
            let folded = fold(text);
            let words: Vec<&str> = folded
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            tasks
                .into_iter()
                .filter(|t| {
                    let title = fold(&t.title);
                    words.iter().all(|w| title.contains(w))
                })
                .collect()
        }
    }
}

fn room(state: &State, ix: &Index, ui: &UiState, now: &Now, ctx: &EvalContext) -> Room {
    let key = ui.room.resolved(state);
    let shared = shared_key(key);
    // The header (progress, summary, eyebrow, counts) comes from the shared derivation, before filtering.
    let tasks =
        room_header::room_tasks_with_updates(state, shared, &ui.holding, &ui.held_updates, ctx);
    let header = room_header::header_of(state, shared, &tasks, &ui.holding, &now.local());
    let room_header::RoomTasks { open, done } = tasks;

    // The filter narrows tasks; Activity has none to narrow.
    let filtered = !ui.filter.trim().is_empty() && !key.is_history();
    let open = filter(state, &ui.filter, ctx, open, &ui.held_updates);
    let done = filter(state, &ui.filter, ctx, done, &ui.held_updates);
    let completed_count = done.len();

    let (title, hue, chips, placeholder): (SharedString, Hue, Chips, SharedString) = match key {
        RoomKey::Smart(s) => (
            s.builtin().name.into(),
            s.hue(),
            Chips::CollectionAndList,
            "Add a task…".into(),
        ),
        RoomKey::Collection(c) => {
            let col = &state.collections[&c];
            (
                labels::one_line(&col.name).into(),
                lists::hue(col.color.as_deref(), c.as_uuid().as_bytes()),
                Chips::List,
                format!("Add a task to {}…", labels::one_line(&col.name)).into(),
            )
        }
        RoomKey::Activity(_) => (
            "Activity".into(),
            hues::AGENT_ACTIVITY,
            Chips::None,
            "".into(),
        ),
        RoomKey::Trash => (
            todo_client::trash::TITLE.into(),
            hues::AGENT_ACTIVITY,
            Chips::None,
            "".into(),
        ),
        RoomKey::List(l) => {
            let list = ix.list(l);
            (
                list.name.clone(),
                list.hue,
                Chips::None,
                format!("Add a task to {}…", list.name).into(),
            )
        }
    };

    let finished = header.finished_day();
    let tomorrow = (finished && !filtered && !ui.compact)
        .then(|| tomorrow(state, ix, now, &ui.holding))
        .flatten();
    // Planning the next day is offered whenever the day is finished, even with nothing due then.
    let plan_weekday = (finished && !filtered && !ui.compact)
        .then(|| add_days(now.today(), 1))
        .flatten()
        .map(|d| SharedString::from(labels::weekday(d)));
    let as_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);

    Room {
        key,
        title,
        eyebrow: eyebrow(header.eyebrow.clone()),
        hue,
        chips,
        features: match key {
            RoomKey::List(id) => state.lists[&id].features,
            _ => Default::default(),
        },
        board: None,
        timeline: None,
        done: as_u32(header.progress_done),
        total: as_u32(header.progress_total),
        score: header.summary,
        status: header.status,
        estimate: None,
        tasks: open.iter().map(|t| row(state, ix, t, now)).collect(),
        trash: if key == RoomKey::Trash {
            todo_client::trash::rows(state, now.at)
        } else {
            Vec::new()
        },
        completed: if ui.show_completed {
            done.iter().map(|t| row(state, ix, t, now)).collect()
        } else {
            Vec::new()
        },
        completed_count: as_u32(completed_count),
        finished_this_month: as_u32(header.finished_this_month),
        done_today: as_u32(header.done_today),
        empty_line: header.empty_line(filtered).into(),
        empty_hint: empty_hint(key, filtered).into(),
        quick_add_placeholder: placeholder,
        finishes_day: header.finishes_day,
        tomorrow,
        plan_weekday,
        filtered,
    }
}

/// How to fill an empty room. The global quick-add (Ctrl+Alt+Space) always adds to My Day, so only My Day
/// mentions it; rooms that only collect tasks say where they come from instead.
fn empty_hint(key: RoomKey, filtered: bool) -> &'static str {
    match key {
        _ if filtered => "Clear the filter to see every task.",
        RoomKey::Smart(Smart::MyDay) => "Add one above, or press Ctrl+Alt+Space from anywhere.",
        RoomKey::Smart(Smart::Completed) => "Tasks show up here when you finish them.",
        RoomKey::Smart(Smart::AgentActivity) => "Tasks your agents add or change show up here.",
        RoomKey::Activity(Some(_)) => "Clear the filter to see every change.",
        RoomKey::Activity(None) => "Changes from you, your agents and email show up here.",
        RoomKey::Trash => "",
        _ => "Add one above; Ctrl+N takes you there.",
    }
}

/// Tomorrow's open tasks and the rest of the week's, for the finished day. A task completed from the
/// hand-over keeps its place (drawn done) while its completion plays (`holding`), as a row does.
fn tomorrow(state: &State, ix: &Index, now: &Now, holding: &BTreeSet<TaskId>) -> Option<Tomorrow> {
    let today = now.today();
    let tomorrow = add_days(today, 1)?;
    let mut soon: Vec<&Task> = state
        .tasks
        .values()
        .filter(|t| (!t.is_done() || holding.contains(&t.id)) && task_live(state, t))
        .filter(|t| {
            t.due
                .is_some_and(|d| (1..=7).contains(&days_between(today, d)))
        })
        .collect();
    soon.sort_by(|a, b| {
        (
            a.due,
            std::cmp::Reverse(a.priority),
            ix.reminder.get(&a.id),
            &a.position,
        )
            .cmp(&(
                b.due,
                std::cmp::Reverse(b.priority),
                ix.reminder.get(&b.id),
                &b.position,
            ))
    });
    let (tmrw, week): (Vec<&Task>, Vec<&Task>) =
        soon.into_iter().partition(|t| t.due == Some(tomorrow));
    let first = tmrw.first()?;
    let first_when = match ix.reminder.get(&first.id) {
        Some(at) if now.date_of(*at) == tomorrow => format!(
            "{}, {}",
            labels::short_date(tomorrow, today),
            labels::clock(*at, &now.tz)
        ),
        _ => labels::short_date(tomorrow, today),
    };
    const ALSO: usize = 2;
    const WEEK: usize = 3;
    let also: Vec<TaskRow> = tmrw
        .iter()
        .skip(1)
        .take(ALSO)
        .map(|t| row(state, ix, t, now))
        .collect();
    let week_rows: Vec<TaskRow> = week
        .iter()
        .take(WEEK)
        .map(|t| row(state, ix, t, now))
        .collect();
    let more = (tmrw.len().saturating_sub(1 + ALSO)) + week.len().saturating_sub(WEEK);
    Some(Tomorrow {
        weekday: labels::weekday(tomorrow).into(),
        first: row(state, ix, first, now),
        first_when: first_when.into(),
        also,
        week: week_rows,
        more: u32::try_from(more).unwrap_or(u32::MAX),
    })
}

fn sidebar(
    state: &State,
    ix: &Index,
    ui: &UiState,
    sync: &SyncInfo,
    now: &Now,
    ctx: &EvalContext,
) -> Sidebar {
    let today = ctx.today;
    let as_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let agent_today = state
        .tasks
        .values()
        .filter(|t| {
            t.updated.by.is_agent() && now.date_of(t.updated.at) == today && task_live(state, t)
        })
        .count();
    let smart = Smart::ALL
        .iter()
        .map(|&s| match s {
            Smart::AgentActivity => NavItem {
                smart: s,
                count: None,
                pip: (agent_today > 0).then(|| format!("{agent_today} new").into()),
            },
            _ => NavItem {
                smart: s,
                count: Some(as_u32(room_header::smart_list_count(
                    state,
                    s.builtin(),
                    ctx,
                ))),
                pip: None,
            },
        })
        .collect();

    let mut open_per_list: HashMap<ListId, u32> = HashMap::new();
    for t in state.tasks.values() {
        if !t.is_done() && t.deleted.is_none() && t.merged_into.is_none() {
            *open_per_list.entry(t.list).or_default() += 1;
        }
    }
    let mut live_lists: Vec<&todo_core::model::List> = state
        .lists
        .values()
        .filter(|l| list_live(state, l.id))
        .collect();
    live_lists.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    let nav = |l: &todo_core::model::List| ListNav {
        id: l.id,
        list: ix.list(l.id),
        glyph: lists::list_glyph(&l.name),
        count: open_per_list.get(&l.id).copied().unwrap_or(0),
    };
    let mut cols: Vec<&todo_core::model::Collection> = state
        .collections
        .values()
        .filter(|c| c.deleted.is_none())
        .collect();
    cols.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    let collections = cols
        .iter()
        .map(|c| {
            let lists: Vec<ListNav> = live_lists
                .iter()
                .filter(|l| l.collection == Some(c.id))
                .map(|l| nav(l))
                .collect();
            let collapsed = ui.collapsed_collections.contains(&c.id);
            let selected = ui.room == RoomKey::Collection(c.id)
                || (collapsed && lists.iter().any(|l| ui.room == RoomKey::List(l.id)));
            CollectionNav {
                id: c.id,
                glyph: lists::collection_glyph(&c.name),
                name: labels::one_line(&c.name).into(),
                hue: lists::hue(c.color.as_deref(), c.id.as_uuid().as_bytes()),
                count: lists.iter().map(|l| l.count).sum(),
                collapsed,
                selected,
                lists: if collapsed { Vec::new() } else { lists },
            }
        })
        .collect();
    let top: Vec<ListNav> = live_lists
        .iter()
        .filter(|l| l.collection.is_none())
        .map(|l| nav(l))
        .collect();

    let counts = agent_counts_today(state, |at| now.date_of(at) == today);
    let agents = select_agent_rows(counts)
        .filter_map(|(actor, added, edited)| {
            Some(AgentRow {
                key: AgentKey::of(actor.agent_id()?)?,
                agent: lists::agent(actor)?,
                name: lists::agent_name(actor)?.into(),
                status: agent_status(added, edited).into(),
            })
        })
        .collect();

    let (sync_text, tone) = sync_line(sync, now);
    Sidebar {
        smart,
        collections,
        lists: top,
        agents,
        selected: ui.room,
        sync: sync_text.full.into(),
        sync_short: sync_text.short.into(),
        tone,
    }
}

fn sync_line(sync: &SyncInfo, now: &Now) -> (todo_client::sync_label::SyncText, Tone) {
    use todo_client::sync_label::SyncLabel;

    let time = sync.synced_at.map(|at| labels::clock(at, &now.tz));
    let label = if sync.paired {
        SyncLabel::for_client(
            Some(sync.connection),
            sync.pending,
            sync.failed,
            sync.unauthorized,
            time.as_deref(),
        )
    } else {
        SyncLabel::Unpaired
    };
    let tone = match label {
        SyncLabel::Synced(_) => Tone::Ok,
        SyncLabel::Syncing => Tone::Busy,
        _ => Tone::Problem,
    };
    (label.text(), tone)
}

fn detail(state: &State, ix: &Index, id: TaskId, now: &Now) -> Option<Detail> {
    let task = state.tasks.get(&id).filter(|t| task_live(state, t))?;
    let today = now.today();
    let created = now.date_of(task.created.at);
    let mut subtitle = format!("Created {}", labels::day_month(created, today));
    if let Some(name) = lists::agent_name(&task.created.by) {
        subtitle = format!("{subtitle} by {name}");
    }
    if task.updated.at != task.created.at {
        let day = now.date_of(task.updated.at);
        let when = if day == today {
            "today".to_string()
        } else {
            labels::day_month(day, today)
        };
        subtitle = format!(
            "{subtitle} / Edited {when} {}",
            labels::clock(task.updated.at, &now.tz)
        );
    }
    let mut steps: Vec<&todo_core::model::Step> = state
        .steps
        .values()
        .filter(|s| s.task == id && s.deleted.is_none())
        .collect();
    steps.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));

    let (due_value, due_kind) = match task.due {
        Some(d) => (labels::short_date(d, today).into(), FieldKind::Date),
        None => ("–".into(), FieldKind::Empty),
    };
    let (priority_name, _) = super::menus::priority_label(task.priority);
    let mut fields = vec![
        Field {
            icon: Icon::Calendar,
            label: "Due",
            value: due_value,
            kind: due_kind,
            edit: Some(FieldEdit::Due),
        },
        Field {
            icon: Icon::Flag,
            label: "Priority",
            value: if task.priority == Priority::None {
                "–".into()
            } else {
                priority_name.into()
            },
            kind: if task.priority == Priority::None {
                FieldKind::Empty
            } else {
                FieldKind::Words
            },
            edit: Some(FieldEdit::Priority),
        },
    ];
    let features = state
        .lists
        .get(&task.list)
        .map_or_default(|list| list.features);
    let status = features
        .workflow
        .then(|| todo_core::query::QueryStatus::of(task));
    if features.timeline {
        fields.insert(
            0,
            Field {
                icon: Icon::CalendarRange,
                label: "Starts",
                value: task
                    .start_date
                    .map_or_else(|| "–".into(), |d| labels::short_date(d, today).into()),
                kind: if task.start_date.is_some() {
                    FieldKind::Date
                } else {
                    FieldKind::Empty
                },
                edit: Some(FieldEdit::Starts),
            },
        );
    }
    if let Some(stage) = status {
        let (label, _) = super::workflow::status(stage);
        fields.insert(
            0,
            Field {
                icon: Icon::Columns3,
                label: "Status",
                value: label.into(),
                kind: FieldKind::Words,
                edit: Some(FieldEdit::Status),
            },
        );
    }
    // Always there, like Repeat: the picker sets, moves or removes the reminder it shows (the first
    // from today, else the latest; `intent::reminder::shown`).
    let reminder = super::commands::reminder::shown(state, id, &now.eval());
    fields.push(Field {
        icon: Icon::Bell,
        label: "Reminder",
        value: reminder.map_or_else(|| "No reminder".into(), |r| labels::when(r.at, now).into()),
        kind: if reminder.is_some() {
            FieldKind::Date
        } else {
            FieldKind::Empty
        },
        edit: Some(FieldEdit::Reminder),
    });
    fields.push(Field {
        icon: Icon::Repeat,
        label: "Repeat",
        value: task
            .recurrence
            .as_ref()
            .map(labels::describe_repeat)
            .unwrap_or_else(|| "Doesn’t repeat".into())
            .into(),
        kind: if task.recurrence.is_some() {
            FieldKind::Words
        } else {
            FieldKind::Empty
        },
        edit: Some(FieldEdit::Repeat),
    });
    let mut tags: Vec<&todo_core::model::Tag> = task
        .tags
        .iter()
        .filter_map(|t| state.tags.get(t))
        .filter(|t| t.deleted.is_none())
        .collect();
    tags.sort_by_cached_key(|t| (todo_core::query::fold(&t.name), t.id));
    let names: Vec<String> = tags
        .iter()
        .map(|t| format!("#{}", labels::one_line(&t.name)))
        .collect();
    // Always there, like Due and Priority: the tag picker opens from it (or T).
    fields.push(Field {
        icon: Icon::Hash,
        label: "Tags",
        value: if names.is_empty() {
            "–".into()
        } else {
            names.join(" ").into()
        },
        kind: if names.is_empty() {
            FieldKind::Empty
        } else {
            FieldKind::Tags
        },
        edit: Some(FieldEdit::Tags),
    });
    let tags = tags
        .iter()
        .map(|t| TagChip {
            id: t.id,
            name: labels::one_line(&t.name).into(),
            hue: super::menus::tag_hue(t),
        })
        .collect();

    let mut links: Vec<&todo_core::model::Link> = state
        .links
        .values()
        .filter(|l| l.task == id && l.deleted.is_none())
        .collect();
    links.sort_by_key(|l| (l.created.at, l.id));
    let links = links
        .iter()
        .map(|l| match &l.kind {
            LinkKind::Web { url, title } => LinkRow {
                id: l.id,
                title: title
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .map_or_else(|| labels::one_line(&host(url)), |t| labels::one_line(&t))
                    .into(),
                detail: labels::one_line(url).into(),
                // Only http(s) opens; anything else is shown as text.
                url: super::files::openable_link(url).map(Into::into),
                email: false,
            },
            LinkKind::Email(e) => LinkRow {
                id: l.id,
                title: labels::one_line(&e.subject).into(),
                detail: format!(
                    "{} · {}",
                    labels::one_line(&e.sender),
                    labels::when(e.received_at, now)
                )
                .into(),
                url: e
                    .web_link
                    .as_deref()
                    .and_then(super::files::openable_link)
                    .map(Into::into),
                email: true,
            },
        })
        .collect();
    let mut attachments: Vec<&todo_core::model::AttachmentRef> = state
        .attachments
        .values()
        .filter(|a| a.task == id && a.deleted.is_none())
        .collect();
    attachments.sort_by_key(|a| (a.created.at, a.id));
    let attachments = attachments
        .iter()
        .map(|a| AttachmentRow {
            id: a.id,
            name: a.file_name.clone().into(),
            label: labels::one_line(&a.file_name).into(),
            detail: labels::size(a.size_bytes).into(),
        })
        .collect();

    Some(Detail {
        task: id,
        list: ix.list(task.list),
        title: task.title.clone().into(),
        subtitle: subtitle.into(),
        activity: todo_client::task_activity::lines(state, id, now.at),
        done: task.is_done(),
        my_day: task.my_day == Some(today),
        important: task.priority == Priority::High,
        priority: task.priority,
        status,
        steps: steps
            .iter()
            .map(|s| StepRow {
                id: s.id,
                title: labels::one_line(&s.title).into(),
                done: s.completed_at.is_some(),
            })
            .collect(),
        notes: task.notes.clone().into(),
        fields,
        tags,
        links,
        attachments,
    })
}

/// "example.com" from a URL, for links without a title.
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_start_matches("www.")
        .to_string()
}

/// The quick-add window observes all shell notifications, including animation frames. Publish its
/// raw My Day counts only for a new replica snapshot or local date, never in the observer.
#[derive(Debug, Default)]
pub struct MyDayProgress {
    snapshot: Weak<State>,
    day: Option<Date>,
    counts: (u32, u32),
}

impl MyDayProgress {
    pub fn counts(&self) -> (u32, u32) {
        self.counts
    }

    /// Returns whether the snapshot or date needed a recount. The replica reuses its Arc until its
    /// view changes; the Weak keeps pointer identity unambiguous without retaining the old data.
    pub fn update(&mut self, snapshot: &Arc<State>, day: Date) -> bool {
        if self.day == Some(day) && std::ptr::eq(self.snapshot.as_ptr(), Arc::as_ptr(snapshot)) {
            return false;
        }
        self.snapshot = Arc::downgrade(snapshot);
        self.day = Some(day);
        self.counts = my_day_progress(snapshot, day);
        true
    }
}

pub fn my_day_progress(state: &State, day: Date) -> (u32, u32) {
    let (done, total) = room_header::my_day_progress(state, day);
    let as_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    (as_u32(done), as_u32(total))
}

/// The state as the list places tasks while their choreography plays (see `UiState::holding`,
/// `reopening` and `leaving`): each task being reopened still done, as it was, each task leaving (deleted
/// or moved away) as it was before, and with `held_open` each task being completed still open. The room
/// places held tasks itself (it draws them done), so only the sidebar's counts use `held_open`; the
/// detail pane always shows the truth.
fn as_placed(state: &State, ui: &UiState, held_open: bool) -> State {
    let mut placed = state.clone();
    for (id, task) in &ui.leaving {
        placed.tasks.insert(*id, task.clone());
    }
    for (id, at) in &ui.reopening {
        if let Some(t) = placed.tasks.get_mut(id)
            && t.completed_at.is_none()
        {
            t.completed_at = Some(*at);
        }
    }
    if held_open {
        for id in &ui.holding {
            if let Some(t) = placed.tasks.get_mut(id) {
                t.completed_at = None;
            }
        }
    }
    placed
}

/// Derives what the window shows.
pub fn derive(state: &State, ui: &UiState, sync: &SyncInfo, now: &Now) -> Shell {
    let ctx = now.eval();
    let ix = Index::new(state, now);
    let (in_room, in_sidebar);
    let room_state = if ui.reopening.is_empty() && ui.leaving.is_empty() {
        state
    } else {
        in_room = as_placed(state, ui, false);
        &in_room
    };
    let sidebar_state = if ui.reopening.is_empty() && ui.holding.is_empty() && ui.leaving.is_empty()
    {
        state
    } else {
        in_sidebar = as_placed(state, ui, true);
        &in_sidebar
    };
    let mut room = room(room_state, &ix, ui, now, &ctx);
    if let RoomKey::List(list) = room.key
        && !ui.compact
        && let Some(board) = todo_client::board::project(sidebar_state, list, &ctx)
    {
        room.board = Some(super::BoardRows {
            total_done: board.total_done,
            columns: board.columns.map(|column| {
                let tasks = column
                    .cards
                    .iter()
                    .filter_map(|id| room_state.tasks.get(id))
                    .collect();
                filter(room_state, &ui.filter, &ctx, tasks, &ui.held_updates)
                    .into_iter()
                    .map(|task| row(room_state, &ix, task, now))
                    .collect()
            }),
        });
    }
    if !ui.compact
        && let Some(projected) =
            todo_client::timeline::room(sidebar_state, shared_key(room.key), &ctx)
    {
        room.features.timeline = true;
        let visible = filter(
            room_state,
            &ui.filter,
            &ctx,
            projected
                .lanes
                .iter()
                .map(|l| l.task)
                .chain(projected.unscheduled.iter().copied())
                .filter_map(|id| room_state.tasks.get(&id))
                .collect(),
            &ui.held_updates,
        );
        let rows: std::collections::HashMap<_, _> =
            visible.into_iter().map(|t| (t.id, t)).collect();
        room.timeline = Some(super::TimelineRows {
            lanes: projected
                .lanes
                .into_iter()
                .filter_map(|lane| {
                    rows.get(&lane.task)
                        .map(|task| (lane, row(room_state, &ix, task, now)))
                })
                .collect(),
            unscheduled: projected
                .unscheduled
                .into_iter()
                .filter_map(|id| {
                    rows.get(&id)
                        .filter(|_| !sidebar_state.tasks[&id].is_done())
                        .map(|task| row(room_state, &ix, task, now))
                })
                .collect(),
        });
    }
    let mut sidebar = sidebar(sidebar_state, &ix, ui, sync, now, &ctx);
    sidebar.selected = room.key;
    let detail = ui.task.and_then(|id| detail(state, &ix, id, now));
    let mut shell = Shell {
        room,
        sidebar,
        detail,
        selected_task: None,
    };
    shell.selected_task = ui.task.and_then(|id| shell.rows().position(|r| r.id == id));
    shell
}

#[cfg(test)]
mod tests;
