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
pub(super) struct Undos {
    outcomes: RwSignal<BTreeMap<EventId, (EventId, Outcome)>>,
    announcement: RwSignal<Option<&'static str>>,
}

impl Undos {
    pub(super) fn new() -> Self {
        Self {
            outcomes: RwSignal::new(BTreeMap::new()),
            announcement: RwSignal::new(None),
        }
    }

    pub(super) fn settle(self) {
        let settled: Vec<_> = self.outcomes.with_untracked(|undos| {
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
            self.outcomes.update(|undos| {
                for (id, refusal) in settled {
                    self.announcement
                        .set(Some(refusal.unwrap_or("Change undone.")));
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
        self.outcomes
            .with(|undos| undos.get(&id).map(|entry| entry.1))
    }

    fn group_outcome(self, group: EventId) -> Option<Outcome> {
        self.outcomes.with(|undos| {
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
        self.outcomes.update(|undos| {
            undos.insert(target.id, (group, outcome));
        });
        self.announcement.set(Some(match outcome {
            Outcome::Pending(_) => "Undoing…",
            Outcome::FixtureDone => "Change undone.",
            Outcome::Refused(message) => message,
        }));
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Load {
    Latest,
    Older,
}

enum AfterLoad {
    Ignore,
    Applied,
    Refresh,
    SignIn,
}

#[derive(Default)]
struct Pages {
    entries: BTreeMap<Reverse<u64>, Entry>,
    next: Option<u64>,
    pending: Option<Load>,
    refresh_queued: bool,
    error: Option<(Option<u64>, ActivityError)>,
    reading: bool,
    revision: Revision,
    request: u64,
}

fn load(c: Ctx, pages: RwSignal<Pages>, agent: Option<AgentId>, before: Option<u64>) {
    let revision = c.activity_revision.get_untracked();
    let kind = if before.is_none() {
        Load::Latest
    } else {
        Load::Older
    };
    let Some(Some(request)) = pages.try_update(|state| {
        if let Some(pending) = state.pending {
            if kind == Load::Latest && pending == Load::Older {
                state.refresh_queued = true;
            }
            return None;
        }
        state.pending = Some(kind);
        state.error = None;
        state.request += 1;
        Some(state.request)
    }) else {
        return;
    };
    c.activity_undos.announcement.set(None);
    leptos::task::spawn_local(async move {
        let result = backend::activity(&ActivityQuery {
            before,
            limit: Some(PAGE_SIZE),
            agent: agent.clone(),
        })
        .await;
        let focused_row = if kind == Load::Latest && result.is_ok() {
            document()
                .active_element()
                .and_then(|el| el.closest(".activity-row").ok().flatten())
        } else {
            None
        };
        let next = pages.try_update(|state| {
            if state.request != request {
                return AfterLoad::Ignore;
            }
            state.pending = None;
            let refresh = std::mem::take(&mut state.refresh_queued);
            match result {
                Err(ActivityError::Unauthorized) => return AfterLoad::SignIn,
                // The refresh replaces all pages. Don't append the stale page just to discard it.
                _ if refresh => return AfterLoad::Refresh,
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
            AfterLoad::Applied
        });
        match next {
            Some(AfterLoad::Refresh) => load(c, pages, agent, None),
            Some(AfterLoad::SignIn) => platform::sign_in(&platform::path()),
            Some(AfterLoad::Applied) => {
                if let Some(row) = focused_row {
                    request_animation_frame(move || {
                        if !row.is_connected() && pages.try_with_untracked(|_| ()).is_some() {
                            platform::refocus_row("room-title");
                        }
                    });
                }
            }
            Some(AfterLoad::Ignore) | None => {}
        }
    });
}

#[component]
pub(super) fn Feed(agent: Option<AgentId>, reached_bottom: RwSignal<u64>) -> impl IntoView {
    let c = ctx();
    let pages = RwSignal::new(Pages::default());
    let announcement = c.activity_undos.announcement;
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
    let pending = move || pages.with(|p| p.pending.is_some());
    let refresh_pending =
        move || pages.with(|p| p.pending == Some(Load::Latest) || p.refresh_queued);
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
                    <button class="button activity-button" type="button" aria-disabled=move || refresh_pending().to_string()
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
                <p class="activity-progress" aria-hidden="true">{move || pending().then_some("Loading activity…")}</p>
                <span class="activity-announcement" role="status">{move || {
                    announcement.get().or_else(|| pending().then_some("Loading activity…"))
                }}</span>
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Undo,
    Undoing,
    Undone,
    Unavailable,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Undo => "Undo",
            Self::Undoing => "Undoing",
            Self::Undone => "Undone",
            Self::Unavailable => "Undo unavailable",
        }
    }
}

#[component]
fn ActivityRow(id: EventId, entry: Memo<Option<Entry>>) -> impl IntoView {
    let c = ctx();
    let undos = c.activity_undos;
    let action = Memo::new(move |_| {
        // The outbox can change without advancing the confirmed cursor.
        c.sync.track();
        entry.with(|item| {
            let Some(row) = item.as_ref() else {
                return Action::Unavailable;
            };
            let group = undos.group_outcome(row.group);
            let flags = backend::activity_flags(id, row.compensating);
            // The feed can be ahead of the replica. Once either knows the link is no longer a
            // head, it cannot be undone again (a redo creates a new link).
            let (undone, can_undo) = match flags.confirmed {
                Some((undone, false)) => (undone, false),
                _ => (row.undone, row.can_undo),
            };
            if group == Some(Outcome::FixtureDone) || undone {
                Action::Undone
            } else if !can_undo {
                Action::Unavailable
            } else if flags.pending || matches!(group, Some(Outcome::Pending(_))) {
                Action::Undoing
            } else {
                Action::Undo
            }
        })
    });
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
                <div class="activity-refusal">{move || match undos.outcome(id) {
                    Some(Outcome::Refused(message)) => Some(message),
                    _ => None,
                }}</div>
            </div>
            <div class="activity-row-action">
                <Show when=move || matches!(action.get(), Action::Undo | Action::Undoing)
                    fallback=move || view! {
                        <span class="activity-undone">{move || action.get().label()}</span>
                    }>
                    <button class="button activity-button" type="button"
                        aria-label=move || entry.with(|item| item.as_ref().map(|row| {
                            format!("{}: {}", action.get().label(), row.what)
                        }))
                        aria-disabled=move || (action.get() != Action::Undo || c.read_only()).to_string()
                        on:click=move |_| {
                            if !c.read_only() && action.get_untracked() == Action::Undo
                                && let Some(row) = entry.get_untracked()
                            {
                                undos.run(c, row.target, row.group);
                            }
                        }>{move || action.get().label()}</button>
                </Show>
            </div>
        </li>
    }
}
