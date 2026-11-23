//! The Activity room. Feed pages are independent of the task replica; actions use its outbox.

use super::{Ctx, RoomId, ctx, icon, link_room, mark, model, you_mark};
use crate::{backend, platform};
use jiff::civil::Date;
use leptos::prelude::*;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use todo_client::Epoch;
use todo_client::activity::{
    self, ActivityQuery, Entry, Feed as Pages, Loaded, UndoAction, UndoOutcome,
};
use todo_client::protocol::EventRef;
use todo_client::room_header::{LocalTime, StatusCounts};
use todo_core::actor::{Actor, AgentId};
use todo_core::ids::EventId;
use todo_design::Icon;

const PAGE_SIZE: usize = 20;
type Revision = Option<(Option<Epoch>, u64)>;

/// The undos asked for from the feed (`activity::Undos`), and what was last said about them.
#[derive(Clone, Copy)]
pub(super) struct Undos {
    outcomes: RwSignal<activity::Undos>,
    announcement: RwSignal<Option<&'static str>>,
}

impl Undos {
    pub(super) fn new() -> Self {
        Self {
            outcomes: RwSignal::new(activity::Undos::default()),
            announcement: RwSignal::new(None),
        }
    }

    pub(super) fn settle(self) {
        if !self.outcomes.with_untracked(activity::Undos::any_pending) {
            return;
        }
        let mut undos = self.outcomes.get_untracked();
        if let Some(said) = undos.settle(backend::undo_result) {
            self.outcomes.set(undos);
            self.announcement.set(Some(said));
        }
    }

    fn refusal(self, id: EventId) -> Option<&'static str> {
        self.outcomes.with(|undos| undos.refusal(id))
    }

    fn group_outcome(self, group: EventId) -> Option<UndoOutcome> {
        self.outcomes.with(|undos| undos.group_outcome(group))
    }

    fn run(self, c: Ctx, target: EventRef, group: EventId) {
        if self.group_outcome(group).is_some() {
            return;
        }
        let outcome = match backend::undo_activity(target) {
            Ok(Some(command)) => UndoOutcome::Pending(command),
            Ok(None) => UndoOutcome::Done,
            Err(message) => UndoOutcome::Refused(message),
        };
        let said = self
            .outcomes
            .try_update(|undos| undos.record(target.id, group, outcome));
        self.announcement.set(said);
        c.refresh();
    }
}

/// The pages read so far (shared with every client), and the replica's position when the newest
/// page was read, for the refresh button's dot.
#[derive(Default)]
pub(super) struct FeedState {
    pages: Pages,
    revision: Revision,
    agent: Option<AgentId>,
}

impl FeedState {
    pub(super) fn counts(&self, agent: Option<&AgentId>, now: &LocalTime) -> StatusCounts {
        if agent != self.agent.as_ref() {
            return StatusCounts::activity(0, 0);
        }
        let today = now.today();
        StatusCounts::activity(
            self.pages.len(),
            self.pages
                .entries()
                .filter(|row| now.date_of(row.at) == today)
                .count(),
        )
    }
}

fn load(c: Ctx, feed: RwSignal<FeedState>, agent: Option<AgentId>, before: Option<u64>) {
    let revision = c.activity_revision.get_untracked();
    let Some(Some(request)) = feed.try_update(|state| state.pages.begin(before)) else {
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
        let fresh = before.is_none() && result.is_ok();
        let focused_row = if fresh {
            document()
                .active_element()
                .and_then(|el| el.closest(".activity-row").ok().flatten())
        } else {
            None
        };
        let next = feed.try_update(|state| {
            let loaded = state.pages.finish(request, before, result);
            if loaded == Loaded::Applied && fresh {
                state.revision = revision;
            }
            loaded
        });
        match next {
            Some(Loaded::Refresh) => load(c, feed, agent, None),
            Some(Loaded::Applied) => {
                if let Some(row) = focused_row {
                    request_animation_frame(move || {
                        if !row.is_connected() && feed.try_with_untracked(|_| ()).is_some() {
                            platform::refocus_row("room-title");
                        }
                    });
                }
            }
            Some(Loaded::SignIn | Loaded::Ignore) | None => {}
        }
    });
}

#[component]
pub(super) fn Feed(agent: Option<AgentId>, reached_bottom: RwSignal<u64>) -> impl IntoView {
    let c = ctx();
    let feed = c.activity_feed;
    // The footer reads the same pages as the list. Drop the previous filter's pages and request.
    feed.set(FeedState {
        agent: agent.clone(),
        ..FeedState::default()
    });
    let announcement = c.activity_undos.announcement;
    let request = Callback::new({
        let agent = agent.clone();
        move |before| load(c, feed, agent.clone(), before)
    });
    // Changing the agent remounts the feed. A replaced log must not continue with an old cursor.
    Effect::new(move |prev: Option<Option<Epoch>>| {
        let epoch = c.activity_revision.get().and_then(|(epoch, _)| epoch);
        if prev.is_none() || prev.flatten().is_some_and(|old| Some(old) != epoch) {
            feed.update(|state| {
                state.pages.reset();
                state.revision = None;
            });
            request.run(None);
        }
        epoch
    });
    Effect::new(move |last: Option<u64>| {
        let bottom = reached_bottom.get();
        if last.is_some_and(|before| before != bottom)
            && let Some(next) = feed.with_untracked(|f| f.pages.older())
        {
            request.run(Some(next));
        }
        bottom
    });
    // Dates group all entries on the same local calendar day, even across page boundaries.
    let days = Memo::new(move |_| {
        let now = c.now.with(model::Now::local_time);
        feed.with(|state| {
            let pages = &state.pages;
            pages
                .days(|at| now.date_of(at))
                .into_iter()
                .map(|(day, seqs)| {
                    let rows = seqs
                        .into_iter()
                        .filter_map(|seq| Some((seq, pages.entry(seq)?.target.id)))
                        .collect();
                    (Reverse(day), rows)
                })
                .collect::<BTreeMap<Reverse<Date>, Vec<(u64, EventId)>>>()
        })
    });
    let has_older = move || feed.with(|f| f.pages.older().is_some());
    let pending = move || feed.with(|f| f.pages.pending().is_some());
    let refresh_pending = move || feed.with(|f| f.pages.refreshing());
    let has_updates = move || {
        let revision = c.activity_revision.get();
        feed.with(|f| f.pages.loaded() && f.revision != revision)
    };
    view! {
        <div class="activity-body">
            <div class="activity-toolbar">
                <span class="caps">{activity::NEWEST_FIRST}</span>
                <div class="activity-actions">
                    {agent.is_some().then(|| view! {
                        <a class="button activity-button" href=RoomId::Activity(None).path()
                            on:click=link_room(c, RoomId::Activity(None))>{activity::CLEAR_FILTER}</a>
                    })}
                    <button class="button activity-button" type="button" aria-disabled=move || refresh_pending().to_string()
                        on:click=move |_| request.run(None)>
                        {activity::REFRESH}
                        <span aria-hidden="true">{move || has_updates().then_some(" •")}</span>
                    </button>
                </div>
            </div>
            <For each=move || days.with(|groups| groups.keys().copied().collect::<Vec<_>>())
                key=|day| *day children=move |day| {
                    view! {
                        <section class="activity-day" aria-labelledby=format!("activity-day-{}", day.0)>
                            <h2 class="caps" id=format!("activity-day-{}", day.0)>{move || {
                                activity::day_label(day.0, c.now.with(model::Now::dates))
                            }}</h2>
                            <ol>
                                <For each=move || days.with(|groups| groups.get(&day).cloned().unwrap_or_default())
                                    key=|key| *key children=move |(seq, id)| {
                                        let entry = Memo::new(move |_| feed.with(|f| f.pages.entry(seq).cloned()));
                                        view! { <ActivityRow id=id entry=entry/> }
                                    }/>
                            </ol>
                        </section>
                    }
                }/>
            <Show when=move || feed.with(|f| f.pages.is_empty_feed())>
                <p class="empty-line">{activity::empty_line(agent.is_some())}</p>
            </Show>
            <div class="activity-page-controls">
                <p class="activity-progress" aria-hidden="true">{move || pending().then_some(activity::LOADING)}</p>
                <span class="activity-announcement" role="status">{move || {
                    announcement.get().or_else(|| pending().then_some(activity::LOADING))
                }}</span>
                {move || feed.with(|f| f.pages.error().zip(f.pages.retry())).map(|(error, next)| {
                    view! {
                        <div class="activity-page-error">
                            <p role="alert">{error.to_string()}</p>
                            <button class="button activity-button" type="button" on:click=move |_| request.run(next)>
                                {activity::retry_label(error)}
                            </button>
                        </div>
                    }
                })}
                <Show when=has_older>
                    <button class="button activity-button" type="button" aria-disabled=move || pending().to_string()
                        on:click=move |_| {
                            if let Some(next) = feed.with_untracked(|f| f.pages.older()) { request.run(Some(next)); }
                        }>{activity::LOAD_OLDER}</button>
                </Show>
            </div>
        </div>
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
                return UndoAction::Unavailable;
            };
            let flags = backend::activity_flags(id, row.compensating);
            activity::undo_action(
                row,
                flags.confirmed,
                flags.pending,
                undos.group_outcome(row.group),
            )
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
                    <span>{move || entry.with(|item| item.as_ref().map(activity::Entry::actor_name))}</span>
                    <span aria-hidden="true">"·"</span>
                    <time datetime=move || entry.with(|item| item.as_ref().map(|row| row.at.to_string()))
                        title=move || entry.with(|item| item.as_ref().map(|row| {
                            let now = c.now.get();
                            let local = now.civil_time_at(row.at);
                            format!("{}, {:02}:{:02}", now.dates().full(local.date()), local.hour(), local.minute())
                        }))>
                        {move || entry.with(|item| item.as_ref().map(|row| activity::relative_time(row.at, c.now.with(|now| now.at))))}
                    </time>
                </div>
                <div class="activity-refusal">{move || undos.refusal(id)}</div>
            </div>
            <div class="activity-row-action">
                <Show when=move || action.get().is_button()
                    fallback=move || view! {
                        <span class="activity-undone">{move || action.get().label()}</span>
                    }>
                    <button class="button activity-button" type="button"
                        aria-label=move || entry.with(|item| item.as_ref().map(|row| {
                            format!("{}: {}", action.get().label(), row.what)
                        }))
                        aria-disabled=move || (action.get() != UndoAction::Undo || c.read_only()).to_string()
                        on:click=move |_| {
                            if !c.read_only() && action.get_untracked() == UndoAction::Undo
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
