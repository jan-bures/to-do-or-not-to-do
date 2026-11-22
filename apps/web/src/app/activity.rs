//! The Activity room. Feed pages are independent of the task replica; actions use its outbox.

use super::{Ctx, RoomId, ctx, icon, link_room, mark, model, you_mark};
use crate::{backend, platform};
use jiff::civil::Date;
use leptos::prelude::*;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use todo_client::activity::{self, ActivityError, ActivityItem, ActivityQuery};
use todo_client::protocol::EventRef;
use todo_client::{CommandId, Epoch};
use todo_core::actor::{Actor, AgentId};
use todo_core::ids::EventId;
use todo_core::time::Timestamp;
use todo_design::Icon;

const PAGE_SIZE: usize = 20;
type Revision = Option<(Option<Epoch>, u64)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pending(CommandId),
    FixtureDone,
    Refused(&'static str),
}

#[derive(Clone, Copy)]
pub(super) struct Undos(RwSignal<BTreeMap<EventId, (EventId, Outcome)>>);

impl Undos {
    pub(super) fn new() -> Self {
        Self(RwSignal::new(BTreeMap::new()))
    }

    pub(super) fn settle(self) {
        let settled: Vec<_> = self.0.with_untracked(|undos| {
            undos
                .iter()
                .filter_map(|(&id, &(_, outcome))| {
                    let Outcome::Pending(command) = outcome else {
                        return None;
                    };
                    backend::undo_result(command).map(|result| (id, result.err()))
                })
                .collect()
        });
        if !settled.is_empty() {
            self.0.update(|undos| {
                for (id, refusal) in settled {
                    if let Some(message) = refusal {
                        if let Some(entry) = undos.get_mut(&id) {
                            entry.1 = Outcome::Refused(message);
                        }
                    } else {
                        // A successful live undo is now in the confirmed journal. Don't keep a
                        // `Done` override: a later undo of the compensation may redo the change.
                        undos.remove(&id);
                    }
                }
            });
        }
    }

    fn outcome(self, id: EventId) -> Option<Outcome> {
        self.0.with(|undos| undos.get(&id).map(|entry| entry.1))
    }

    fn group_outcome(self, group: EventId) -> Option<Outcome> {
        self.0.with(|undos| {
            undos.values().find_map(|&(g, outcome)| {
                (group == g && !matches!(outcome, Outcome::Refused(_))).then_some(outcome)
            })
        })
    }

    fn run(self, c: Ctx, target: EventRef, group: EventId) {
        if self.group_outcome(group).is_some() {
            return;
        }
        let outcome = match backend::undo_activity(target) {
            Ok(Some(command)) => Outcome::Pending(command),
            Ok(None) => Outcome::FixtureDone,
            Err(message) => Outcome::Refused(message),
        };
        self.0.update(|undos| {
            undos.insert(target.id, (group, outcome));
        });
        c.refresh();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    target: EventRef,
    group: EventId,
    actor: Actor,
    at: Timestamp,
    what: String,
    compensating: bool,
    undone: bool,
    can_undo: bool,
}

impl Entry {
    fn from_item(item: ActivityItem) -> Option<Self> {
        let target = item.event_ref()?;
        let what = item.what_happened().to_owned();
        let event = item.event.event;
        Some(Self {
            target,
            group: event.group,
            actor: event.actor,
            at: event.at,
            what,
            compensating: event.undoes.is_some(),
            undone: item.undone,
            can_undo: item.can_undo,
        })
    }
}

#[derive(Default)]
struct Pages {
    entries: BTreeMap<Reverse<u64>, Entry>,
    next: Option<u64>,
    pending: bool,
    error: Option<(Option<u64>, ActivityError)>,
    reading: bool,
    revision: Revision,
    request: u64,
}

fn load(c: Ctx, pages: RwSignal<Pages>, agent: Option<AgentId>, before: Option<u64>) {
    let revision = c.activity_revision.get_untracked();
    let Some(Some(request)) = pages.try_update(|state| {
        if state.pending {
            return None;
        }
        state.pending = true;
        state.error = None;
        state.request += 1;
        Some(state.request)
    }) else {
        return;
    };
    leptos::task::spawn_local(async move {
        let result = backend::activity(&ActivityQuery {
            before,
            limit: Some(PAGE_SIZE),
            agent,
        })
        .await;
        let applied = pages.try_update(|state| {
            if state.request != request {
                return false;
            }
            state.pending = false;
            match result {
                Ok(page) => {
                    if before.is_none() {
                        state.entries.clear();
                        state.revision = revision;
                    }
                    state.reading = true;
                    // A cursor must advance even on a page with no entries visible to the caller.
                    state.next = page
                        .next_before
                        .filter(|next| before.is_none_or(|n| *next < n));
                    for entry in page.items.into_iter().filter_map(Entry::from_item) {
                        state.entries.insert(Reverse(entry.target.seq), entry);
                    }
                }
                Err(error) => state.error = Some((before, error)),
            }
            true
        });
        if applied == Some(true)
            && pages.with_untracked(|p| {
                p.error
                    .is_some_and(|(_, error)| error == ActivityError::Unauthorized)
            })
        {
            platform::sign_in(&platform::path());
        }
    });
}

#[component]
pub(super) fn Feed(agent: Option<AgentId>, reached_bottom: RwSignal<u64>) -> impl IntoView {
    let c = ctx();
    let pages = RwSignal::new(Pages::default());
    let request = Callback::new({
        let agent = agent.clone();
        move |before| load(c, pages, agent.clone(), before)
    });
    // Changing the agent remounts the feed. A replaced log must not continue with an old cursor.
    Effect::new(move |prev: Option<Option<Epoch>>| {
        let epoch = c.activity_revision.get().and_then(|(epoch, _)| epoch);
        if prev.is_none() || prev.flatten().is_some_and(|old| Some(old) != epoch) {
            pages.update(|state| {
                *state = Pages {
                    request: state.request,
                    ..Pages::default()
                }
            });
            request.run(None);
        }
        epoch
    });
    Effect::new(move |last: Option<u64>| {
        let bottom = reached_bottom.get();
        if last.is_some_and(|before| before != bottom)
            && let Some(next) =
                pages.with_untracked(|p| p.error.is_none().then_some(p.next).flatten())
        {
            request.run(Some(next));
        }
        bottom
    });
    // Dates group all entries on the same local calendar day, even across page boundaries.
    let days = Memo::new(move |_| {
        let offset = c.now.with(|n| n.offset_s);
        pages.with(|state| {
            let mut days: BTreeMap<Reverse<Date>, Vec<(u64, EventId)>> = BTreeMap::new();
            for entry in state.entries.values() {
                let day = model::local(entry.at, offset).date();
                days.entry(Reverse(day))
                    .or_default()
                    .push((entry.target.seq, entry.target.id));
            }
            days
        })
    });
    let has_older = move || pages.with(|p| p.next.is_some() && p.error.is_none());
    let pending = move || pages.with(|p| p.pending);
    let has_updates = move || {
        let revision = c.activity_revision.get();
        pages.with(|p| p.reading && p.revision != revision)
    };
    view! {
        <div class="activity-body">
            <div class="activity-toolbar">
                <span class="caps">"Newest first"</span>
                <div class="activity-actions">
                    {agent.is_some().then(|| view! {
                        <a class="button activity-button" href=RoomId::Activity(None).path()
                            on:click=link_room(c, RoomId::Activity(None))>"Clear filter"</a>
                    })}
                    <button class="button activity-button" type="button" aria-disabled=move || pending().to_string()
                        on:click=move |_| request.run(None)>
                        "Refresh activity"
                        <span aria-hidden="true">{move || has_updates().then_some(" •")}</span>
                    </button>
                </div>
            </div>
            <For each=move || days.with(|groups| groups.keys().copied().collect::<Vec<_>>())
                key=|day| *day children=move |day| {
                    view! {
                        <section class="activity-day" aria-labelledby=format!("activity-day-{}", day.0)>
                            <h2 class="caps" id=format!("activity-day-{}", day.0)>{move || {
                                let today = c.now.with(model::Now::today);
                                if day.0 == today { "Today".to_owned() }
                                else if Some(day.0) == today.yesterday().ok() { "Yesterday".to_owned() }
                                else { model::short_date(day.0, today) }
                            }}</h2>
                            <ol>
                                <For each=move || days.with(|groups| groups.get(&day).cloned().unwrap_or_default())
                                    key=|key| *key children=move |(seq, id)| {
                                        let entry = Memo::new(move |_| pages.with(|p| p.entries.get(&Reverse(seq)).cloned()));
                                        view! { <ActivityRow id=id entry=entry/> }
                                    }/>
                            </ol>
                        </section>
                    }
                }/>
            <Show when=move || pages.with(|p| p.reading && p.entries.is_empty() && p.next.is_none() && p.error.is_none())>
                <p class="empty-line">{if agent.is_some() { "No activity from this agent." } else { "No activity yet." }}</p>
            </Show>
            <div class="activity-page-controls">
                <p class="activity-progress" role="status">{move || pending().then_some("Loading activity…")}</p>
                {move || pages.with(|p| p.error).map(|(before, error)| {
                    let next = match error { ActivityError::TooLarge { next_before } => Some(next_before), _ => before };
                    view! {
                        <div class="activity-page-error">
                            <p role="alert">{error.to_string()}</p>
                            <button class="button activity-button" type="button" on:click=move |_| request.run(next)>
                                {if matches!(error, ActivityError::TooLarge { .. }) { "Skip this change" } else { "Try again" }}
                            </button>
                        </div>
                    }
                })}
                <Show when=has_older>
                    <button class="button activity-button" type="button" aria-disabled=move || pending().to_string()
                        on:click=move |_| {
                            if let Some(next) = pages.with_untracked(|p| p.next) { request.run(Some(next)); }
                        }>"Load older activity"</button>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn ActivityRow(id: EventId, entry: Memo<Option<Entry>>) -> impl IntoView {
    let c = ctx();
    let undos = c.activity_undos;
    let group_outcome =
        move || entry.with(|item| item.as_ref().and_then(|row| undos.group_outcome(row.group)));
    let in_progress = move || matches!(group_outcome(), Some(Outcome::Pending(_)));
    let flags = Memo::new(move |_| {
        c.activity_revision.track();
        entry.with(|item| {
            item.as_ref().map_or((false, false), |row| {
                if group_outcome() == Some(Outcome::FixtureDone) {
                    return (true, false);
                }
                match backend::activity_flags(id, row.compensating) {
                    // Old links never become heads again: a redo adds a new link. Once the
                    // replica sees a compensation, its journal can also distinguish an undone
                    // original from one that's since been redone.
                    Some((undone, false)) => (undone, false),
                    // The page can be ahead of the replica; never re-enable a superseded link.
                    _ => (row.undone, row.can_undo),
                }
            })
        })
    });
    let had_button = entry.with_untracked(|item| item.as_ref().is_some_and(|row| row.can_undo));
    view! {
        <li class="activity-row">
            <span class="activity-mark" aria-hidden="true">{move || entry.with(|item| {
                match item.as_ref().map(|row| &row.actor) {
                    Some(actor @ Actor::Agent { .. }) => model::agent_mark(actor).map(|m| mark(&m)).into_any(),
                    Some(Actor::Owner { .. }) => you_mark().into_any(),
                    Some(Actor::EmailIngest) => icon(Icon::Mail).into_any(),
                    _ => icon(Icon::Clock).into_any(),
                }
            })}</span>
            <div class="activity-copy">
                <p class="activity-summary">{move || entry.with(|item| item.as_ref().map(|row| row.what.clone()))}</p>
                <div class="activity-meta">
                    <span>{move || entry.with(|item| item.as_ref().map(|row| activity::actor_name(&row.actor)))}</span>
                    <span aria-hidden="true">"·"</span>
                    <time datetime=move || entry.with(|item| item.as_ref().map(|row| row.at.to_string()))
                        title=move || entry.with(|item| item.as_ref().map(|row| {
                            let local = model::local(row.at, c.now.with(|now| now.offset_s));
                            format!("{}, {:02}:{:02}", local.date(), local.hour(), local.minute())
                        }))>
                        {move || entry.with(|item| item.as_ref().map(|row| activity::relative_time(row.at, c.now.with(|now| now.at))))}
                    </time>
                </div>
                <div class="activity-refusal" role="status">{move || match undos.outcome(id) {
                    Some(Outcome::Refused(message)) => Some(message),
                    _ => None,
                }}</div>
            </div>
            <div class="activity-row-action">
                <Show when=move || had_button fallback=move || view! {
                    <span class="activity-undone">{move || if flags.get().0 { "Undone" } else { "Superseded" }}</span>
                }>
                    <button class="button activity-button" class:activity-undone=move || !flags.get().1 type="button"
                        aria-label=move || flags.get().1.then(|| entry.with(|item| item.as_ref().map_or_else(
                            || "Undo".to_owned(), |row| format!("Undo: {}", row.what))))
                        aria-disabled=move || (in_progress() || c.read_only() || !flags.get().1).to_string()
                        tabindex=move || if flags.get().1 { "0" } else { "-1" }
                        on:click=move |_| {
                            if !c.read_only() && flags.get_untracked().1
                                && let Some(row) = entry.get_untracked()
                            {
                                undos.run(c, row.target, row.group);
                            }
                        }>{move || match flags.get() {
                            (true, false) => "Undone",
                            (false, false) => "Superseded",
                            (_, true) if in_progress() => "Undoing…",
                            _ => "Undo",
                        }}</button>
                </Show>
            </div>
        </li>
    }
}
