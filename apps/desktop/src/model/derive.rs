//! `State` → [`Shell`].
//!
//! Progress in the header: in My Day it is the day's tasks, done of all; anywhere else it is today's work,
//! tasks completed today of those plus the open ones, so a list with years of finished tasks doesn't
//! show a sun that has long set.

use std::collections::{BTreeMap, HashMap, HashSet};

use gpui::SharedString;
use todo_core::State;
use todo_core::actor::Actor;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{LinkKind, Priority, Task};
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

/// A live list: not deleted, and not in a deleted collection.
pub(super) fn list_live(state: &State, id: ListId) -> bool {
    state.lists.get(&id).is_some_and(|l| {
        l.deleted.is_none()
            && l.collection.is_none_or(|c| {
                state
                    .collections
                    .get(&c)
                    .is_some_and(|c| c.deleted.is_none())
            })
    })
}

/// A live task: not deleted or merged, in a live list. `State::is_live` without its clones.
pub(super) fn task_live(state: &State, t: &Task) -> bool {
    t.deleted.is_none() && t.merged_into.is_none() && list_live(state, t.list)
}

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
    let due = match labels::due(task.due, ix.reminder.get(&task.id).copied(), now) {
        DueLabel::None => Due::None,
        DueLabel::Soon(s) => Due::Soon(s.into()),
        DueLabel::Plain(s) => Due::Plain(s.into()),
        DueLabel::Overdue(s) => Due::Overdue(s.into()),
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
    }
}

/// The room's tasks: (rows in order, the Completed section in order).
fn room_tasks<'a>(
    state: &'a State,
    key: RoomKey,
    holding: &BTreeSet<TaskId>,
    ctx: &EvalContext,
) -> (Vec<&'a Task>, Vec<&'a Task>) {
    let held = |t: &Task| holding.contains(&t.id) && t.is_done();
    match key {
        RoomKey::Smart(smart) => {
            let Ok(q) = smart.builtin().parse() else {
                return (Vec::new(), Vec::new());
            };
            let m = q.matcher(state, ctx);
            let mut open: Vec<&Task> = state
                .tasks
                .values()
                .filter(|t| {
                    m.matches(t)
                        || (held(t) && task_live(state, t) && {
                            // Still in the room as if it were open, until the completion has played.
                            let mut as_open = (*t).clone();
                            as_open.completed_at = None;
                            q.matches(state, &as_open, ctx)
                        })
                })
                .collect();
            open.sort_by_cached_key(|t| q.order_key(state, t));
            let done = smart
                .done_query()
                .and_then(|s| Query::parse(s).ok())
                .map(|dq| {
                    dq.run(state, ctx)
                        .into_iter()
                        .filter(|t| !holding.contains(&t.id))
                        .collect()
                })
                .unwrap_or_default();
            (open, done)
        }
        RoomKey::List(_) | RoomKey::Collection(_) => {
            let in_room = |t: &Task| match key {
                RoomKey::List(l) => t.list == l,
                RoomKey::Collection(c) => {
                    state.lists.get(&t.list).and_then(|l| l.collection) == Some(c)
                }
                RoomKey::Smart(_) => false,
            };
            let list_pos = |t: &Task| state.lists.get(&t.list).map(|l| l.position.clone());
            let mut open = Vec::new();
            let mut done = Vec::new();
            for t in state.tasks.values() {
                if !in_room(t) || !task_live(state, t) {
                    continue;
                }
                if !t.is_done() || held(t) {
                    open.push(t);
                } else {
                    done.push(t);
                }
            }
            open.sort_by(|a, b| {
                (list_pos(a), &a.position, a.id).cmp(&(list_pos(b), &b.position, b.id))
            });
            done.sort_by(|a, b| b.completed_at.cmp(&a.completed_at).then(a.id.cmp(&b.id)));
            (open, done)
        }
    }
}

/// The inline filter: query language when it parses, else words in the title.
fn filter<'a>(state: &State, text: &str, ctx: &EvalContext, tasks: Vec<&'a Task>) -> Vec<&'a Task> {
    let text = text.trim();
    if text.is_empty() {
        return tasks;
    }
    match Query::parse(text) {
        Ok(q) => tasks
            .into_iter()
            .filter(|t| q.matches(state, t, ctx))
            .collect(),
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

fn completed_on(now: &Now, t: &Task, pred: impl Fn(jiff::civil::Date) -> bool) -> bool {
    t.completed_at.is_some_and(|c| pred(now.date_of(c)))
}

fn room(state: &State, ix: &Index, ui: &UiState, now: &Now, ctx: &EvalContext) -> Room {
    let today = ctx.today;
    let key = ui.room.resolved(state);
    let (open, done) = room_tasks(state, key, &ui.holding, ctx);
    // A task still playing its completion counts as open: the counts (and the sun) change when it has
    // left the rows.
    let counted_done = |t: &Task| t.is_done() && !ui.holding.contains(&t.id);
    let finished_on = |t: &Task, pred: &dyn Fn(jiff::civil::Date) -> bool| {
        counted_done(t) && completed_on(now, t, pred)
    };

    // Progress, before filtering.
    let (progress_done, progress_total) = match key {
        RoomKey::Smart(Smart::MyDay) => {
            let d = open.iter().filter(|t| counted_done(t)).count() + done.len();
            (d, open.len() + done.len())
        }
        RoomKey::Smart(Smart::AgentActivity) => {
            (open.iter().filter(|t| counted_done(t)).count(), open.len())
        }
        _ => {
            let today_done = open
                .iter()
                .chain(done.iter())
                .filter(|t| finished_on(t, &|d| d == today))
                .count();
            let still_open = open.iter().filter(|t| !counted_done(t)).count();
            (today_done, today_done + still_open)
        }
    };
    // The status bar's "done today": only what was completed since local midnight, in every room (My Day's
    // header counts its whole plan, which can hold a task finished on an earlier day).
    let done_today = open
        .iter()
        .chain(done.iter())
        .filter(|t| finished_on(t, &|d| d == today))
        .count();
    let finished_this_month = open
        .iter()
        .chain(done.iter())
        .filter(|t| {
            finished_on(t, &|d| {
                d.year() == today.year() && d.month() == today.month()
            })
        })
        .count();

    let filtered = !ui.filter.trim().is_empty();
    let open = filter(state, &ui.filter, ctx, open);
    let done = filter(state, &ui.filter, ctx, done);
    let completed_count = done.len();

    let (title, eyebrow, hue, chips, empty_line, placeholder): (
        SharedString,
        Eyebrow,
        Hue,
        Chips,
        SharedString,
        SharedString,
    ) = match key {
        RoomKey::Smart(s) => (
            s.builtin().name.into(),
            Eyebrow::Day {
                weekday: labels::weekday(today).into(),
                date: labels::long_date(today).into(),
                week: labels::iso_week(today).into(),
            },
            s.hue(),
            Chips::CollectionAndList,
            s.empty_line().into(),
            "Add a task…".into(),
        ),
        RoomKey::Collection(c) => {
            let col = &state.collections[&c];
            let n = state
                .lists
                .values()
                .filter(|l| l.collection == Some(c) && l.deleted.is_none())
                .count();
            (
                labels::one_line(&col.name).into(),
                Eyebrow::Kind {
                    kind: "Collection".into(),
                    detail: format!("{n} {}", if n == 1 { "list" } else { "lists" }).into(),
                },
                lists::hue(col.color.as_deref(), c.as_uuid().as_bytes()),
                Chips::List,
                "Nothing on the shelf yet.".into(),
                format!("Add a task to {}…", labels::one_line(&col.name)).into(),
            )
        }
        RoomKey::List(l) => {
            let list = ix.list(l);
            let still_open = progress_total - progress_done;
            (
                list.name.clone(),
                Eyebrow::Kind {
                    kind: list.collection.clone().unwrap_or_else(|| "List".into()),
                    detail: format!("{still_open} open").into(),
                },
                list.hue,
                Chips::None,
                "Nothing on the shelf yet.".into(),
                format!("Add a task to {}…", list.name).into(),
            )
        }
    };

    let finishes_day = key == RoomKey::Smart(Smart::MyDay);
    let finished = finishes_day && progress_total > 0 && progress_done >= progress_total;
    let tomorrow = (finished && !filtered)
        .then(|| tomorrow(state, ix, now, &ui.holding))
        .flatten();
    // Planning the next day is offered whenever the day is finished, even with nothing due then.
    let plan_weekday = (finished && !filtered)
        .then(|| add_days(now.today(), 1))
        .flatten()
        .map(|d| SharedString::from(labels::weekday(d)));

    Room {
        key,
        title,
        eyebrow,
        hue,
        chips,
        done: u32::try_from(progress_done).unwrap_or(u32::MAX),
        total: u32::try_from(progress_total).unwrap_or(u32::MAX),
        estimate: None,
        tasks: open.iter().map(|t| row(state, ix, t, now)).collect(),
        completed: if ui.show_completed {
            done.iter().map(|t| row(state, ix, t, now)).collect()
        } else {
            Vec::new()
        },
        completed_count: u32::try_from(completed_count).unwrap_or(u32::MAX),
        finished_this_month: u32::try_from(finished_this_month).unwrap_or(u32::MAX),
        done_today: u32::try_from(done_today).unwrap_or(u32::MAX),
        empty_line: if filtered {
            "Nothing matches the filter.".into()
        } else {
            empty_line
        },
        empty_hint: empty_hint(key, filtered).into(),
        quick_add_placeholder: placeholder,
        finishes_day,
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
    let count = |s: &str| {
        Query::parse(s)
            .map(|q| {
                let m = q.matcher(state, ctx);
                state.tasks.values().filter(|t| m.matches(t)).count()
            })
            .unwrap_or(0)
    };
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
                count: Some(as_u32(count(s.builtin().query))),
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

    // Agents seen this week, with what they did today.
    let week_ago = add_days(today, -7).unwrap_or(today);
    let mut agents: BTreeMap<String, (Actor, u32, u32)> = BTreeMap::new();
    for t in state.tasks.values().filter(|t| t.deleted.is_none()) {
        for (stamp, created) in [(&t.created, true), (&t.updated, false)] {
            let Some(id) = stamp.by.agent_id() else {
                continue;
            };
            let day = now.date_of(stamp.at);
            if day < week_ago {
                continue;
            }
            let e = agents
                .entry(id.as_str().to_owned())
                .or_insert((stamp.by.clone(), 0, 0));
            if day == today {
                if created {
                    e.1 += 1;
                } else if t.created.at != t.updated.at {
                    e.2 += 1;
                }
            }
        }
    }
    let agents = agents
        .values()
        .filter_map(|(actor, added, edited)| {
            let status = match (added, edited) {
                (0, 0) => "idle".to_string(),
                (a, 0) => format!("{a} added"),
                (0, e) => format!("{e} edited"),
                (a, e) => format!("{a} added, {e} edited"),
            };
            Some(AgentRow {
                agent: lists::agent(actor)?,
                name: lists::agent_name(actor)?.into(),
                status: status.into(),
            })
        })
        .take(4)
        .collect();

    let (sync_text, tone) = sync_line(sync, now);
    Sidebar {
        smart,
        collections,
        lists: top,
        agents,
        selected: ui.room,
        sync: sync_text.into(),
        tone,
    }
}

fn sync_line(sync: &SyncInfo, now: &Now) -> (String, Tone) {
    if !sync.paired {
        return ("Not signed in".into(), Tone::Problem);
    }
    if sync.unauthorized {
        return ("Signed out by the server".into(), Tone::Problem);
    }
    if sync.failed > 0 {
        return (format!("{} couldn't sync", sync.failed), Tone::Problem);
    }
    match sync.connection {
        Connection::Offline if sync.pending > 0 => {
            (format!("Offline, {} waiting", sync.pending), Tone::Problem)
        }
        Connection::Offline => ("Offline".into(), Tone::Problem),
        Connection::Syncing => ("Syncing…".into(), Tone::Busy),
        Connection::Online => (
            sync.synced_at.map_or_else(
                || "Synced".to_string(),
                |at| format!("Synced {}", labels::clock(at, &now.tz)),
            ),
            Tone::Ok,
        ),
    }
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
    let mut reminders: Vec<&todo_core::model::Reminder> = state
        .reminders
        .values()
        .filter(|r| r.task == id && r.deleted.is_none())
        .collect();
    reminders.sort_by_key(|r| r.at);
    let upcoming = reminders
        .iter()
        .find(|r| now.date_of(r.at) >= today)
        .or(reminders.last());
    if let Some(r) = upcoming {
        fields.push(Field {
            icon: Icon::Bell,
            label: "Reminder",
            value: labels::when(r.at, now).into(),
            kind: FieldKind::Date,
            edit: None,
        });
    }
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
        done: task.is_done(),
        my_day: task.my_day == Some(today),
        important: task.priority == Priority::High,
        priority: task.priority,
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

/// My Day's progress (done, total) for the quick-add window, without deriving a whole shell.
pub fn my_day_progress(state: &State, now: &Now) -> (u32, u32) {
    let today = now.today();
    let (mut done, mut total) = (0u32, 0u32);
    for t in state.tasks.values() {
        if t.my_day == Some(today) && task_live(state, t) {
            total += 1;
            if t.is_done() {
                done += 1;
            }
        }
    }
    (done, total)
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
    let room = room(room_state, &ix, ui, now, &ctx);
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
