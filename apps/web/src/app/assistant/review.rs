//! Render only trusted, simulated effects and approve exactly the preview made visible to the owner.

use super::{CHAT, TaskLink, assistant_mark, backend, capitalize, ctx, icon, mark, platform};
use leptos::prelude::*;
use std::collections::BTreeSet;
use todo_client::{
    TrackedUndo,
    assistant::{
        ApprovalOutcome, Edit, Effect, ListRef, ProposalCard, ProposalItem, ProposalKind,
        ProposalStatus,
    },
};
use todo_core::{command::Placement, ids::TaskId, model::WorkflowStatus};
use todo_design::Icon;
use uuid::Uuid;

#[component]
pub(super) fn Proposal(turn: Uuid) -> impl IntoView {
    let ai = ctx().ai;
    let review_each = RwSignal::new(false);
    let cursor = RwSignal::new(0usize);
    let card = Memo::new(move |_| {
        ai.view.with(|v| {
            v.turns
                .iter()
                .find(|t| t.id == turn)
                .and_then(|t| t.proposal.clone())
        })
    });
    view! {{move || card.get().map(|card| match card.status {
        ProposalStatus::Pending => view! { <Pending card review_each cursor/> }.into_any(),
        ProposalStatus::Stale => view! {
            <div class="ai-error" role="status">
                "The task snapshot changed. Request a new proposal."
                <button type="button" class="ai-link" on:click=move |_| {
                    if let Some(prompt) = ai.view.with_untracked(|v| v.turns.iter().find(|t| t.id == turn).map(|t| t.prompt.clone())) {
                        ai.draft.set(prompt);
                        ai.focus_composer();
                    }
                }>"Refresh proposal"</button>
                <button type="button" class="ai-link" on:click=move |_| {
                    let _ = CHAT.with(|chat| chat.discard(card.preview.id, card.preview.digest));
                    ai.refresh();
                }>"Reject"</button>
            </div>
        }.into_any(),
        ProposalStatus::Submitted => {
            let tasks = card.preview.items.iter().flat_map(|item| &item.effects).map(|effect| task(effect).id).collect::<BTreeSet<_>>();
            view! { <Receipt id=card.preview.id count=card.preview.items.len() tasks=tasks.into_iter().collect()/> }.into_any()
        },
        status => view! {
            <p class="ai-receipt" role="status">
                {if status == ProposalStatus::Declined { "Proposal rejected" } else { "Superseded by a newer request" }}
            </p>
        }.into_any(),
    })}}
}

fn kind_info(kind: ProposalKind) -> (Icon, &'static str, &'static str) {
    match kind {
        ProposalKind::Create => (Icon::Plus, "New task", "create"),
        ProposalKind::Edit => (Icon::Pencil, "Edit task", "edit"),
        ProposalKind::Complete => (Icon::CircleCheck, "Change completion", "edit"),
        ProposalKind::Destructive => (Icon::TrashTwo, "Delete task", "destructive"),
    }
}

fn task(effect: &Effect) -> &todo_client::assistant::TaskRef {
    match effect {
        Effect::Created { task, .. } | Effect::Edited { task, .. } | Effect::Deleted { task } => {
            task
        }
    }
}

#[component]
fn Pending(
    card: ProposalCard,
    review_each: RwSignal<bool>,
    cursor: RwSignal<usize>,
) -> impl IntoView {
    let c = ctx();
    let ai = c.ai;
    let id = card.preview.id;
    let digest = card.preview.digest;
    let batch = card.available.len() > 1;
    let selected = RwSignal::new(
        card.preview
            .items
            .iter()
            .map(|item| item.index)
            .collect::<BTreeSet<_>>(),
    );
    let info = card
        .preview
        .kind()
        .map_or_else(|| kind_info(ProposalKind::Create), kind_info);
    let first = card
        .preview
        .items
        .iter()
        .flat_map(|item| &item.effects)
        .next()
        .map(task);
    let label = if batch {
        if card.available.iter().all(|item| {
            item.effects
                .iter()
                .all(|effect| effect.kind() == ProposalKind::Create)
        }) {
            format!("{} new tasks", card.available.len())
        } else {
            format!("{} proposed changes", card.available.len())
        }
    } else {
        info.1.into()
    };
    let accessible = if batch {
        format!("{} proposed changes", card.available.len())
    } else {
        first.map_or_else(|| label.clone(), |t| format!("{label}: {}", t.title))
    };
    let list = (!batch).then(|| first.map(|t| t.list.clone())).flatten();
    let error = RwSignal::new(None::<String>);
    let ready = move || !c.read_only() && selected.with(|s| !s.is_empty());
    let select = move |index, checked| {
        let mut next = selected.get_untracked();
        if checked {
            next.insert(index);
        } else {
            next.remove(&index);
        }
        if next.is_empty() {
            error.set(None);
            selected.set(next);
            return;
        }
        if ai.reload_profile() {
            return;
        }
        ai.sync(false);
        let result = CHAT.with(|chat| chat.select_items(id, digest, &next));
        match result {
            Ok(()) => {
                ai.refresh();
                request_animation_frame(move || {
                    platform::focus(&format!("ai-select-{index}"));
                });
            }
            Err(err) => {
                // Reset the browser checkbox, which changed before its `change` event fired.
                selected.update(|_| {});
                error.set(Some(format!("{}.", capitalize(err.to_string()))));
                ai.refresh();
            }
        }
    };
    let approve = move |_| {
        if selected.with_untracked(BTreeSet::is_empty) || ai.reload_profile() {
            return;
        }
        ai.sync(false);
        match CHAT.with(|chat| backend::approve_assistant(chat, id, digest)) {
            Ok(approval) => {
                ai.approvals.update(|receipts| {
                    receipts.insert(id, approval);
                });
                ai.refresh();
                c.refresh();
                ai.focus_composer();
            }
            Err(err) => {
                error.set(Some(format!("{}.", capitalize(err.to_string()))));
                ai.refresh();
            }
        }
    };
    let reject = move |_| {
        if CHAT.with(|chat| chat.discard(id, digest)).is_ok() {
            ai.refresh();
            ai.focus_composer();
        }
    };
    let count = card.available.len();
    let items = card
        .available
        .into_iter()
        .map(|original| {
            let exact = card
                .preview
                .items
                .iter()
                .find(|item| item.index == original.index)
                .cloned()
                .unwrap_or(original);
            let title = exact
                .effects
                .first()
                .map(task)
                .map(|t| t.title.clone())
                .unwrap_or_default();
            (exact, title)
        })
        .collect::<Vec<_>>();
    view! {
        <section class="ai-proposal" id="assistant-pending-proposal" role="region" tabindex="-1"
            aria-label=accessible data-kind=info.2>
            <header class="ai-proposal-header">{icon(info.0)}<h3>{label}</h3>{list.map(|list| view! { <ListChip list/> })}</header>
            {if batch {
                view! {
                    <div class="ai-review-mode">
                        <button type="button" class="button" aria-pressed=move || (!review_each.get()).to_string()
                            on:click=move |_| review_each.set(false)>"Summary"</button>
                        <button type="button" class="button" aria-pressed=move || review_each.get().to_string()
                            on:click=move |_| review_each.set(true)>"Review each"</button>
                    </div>
                    <div class="ai-batch-nav" hidden=move || !review_each.get()>
                        <button type="button" class="icon-button" aria-label="Previous change" disabled=move || cursor.get() == 0
                            on:click=move |_| cursor.update(|n| *n = n.saturating_sub(1))>{icon(Icon::ChevronLeft)}</button>
                        <span>{move || format!("{} of {count}", cursor.get() + 1)}</span>
                        <button type="button" class="icon-button" aria-label="Next change" disabled=move || cursor.get() + 1 >= count
                            on:click=move |_| cursor.update(|n| *n = (*n + 1).min(count - 1))>{icon(Icon::ChevronRight)}</button>
                    </div>
                    <div class="ai-batch">
                        {items.into_iter().enumerate().map(|(position, (item, title))| {
                            let index = item.index;
                            let full_item = item.clone();
                            let summary_title = title.clone();
                            let meta = item.effects.first().map(task).cloned();
                            view! {
                                <div class="ai-batch-row" hidden=move || review_each.get() && cursor.get() != position>
                                    <input type="checkbox" id=format!("ai-select-{index}") title=title.clone()
                                        aria-label=format!("Include {title}") prop:checked=move || selected.with(|ids| ids.contains(&index))
                                        on:change=move |ev| select(index, event_target_checked(&ev))/>
                                    <div class="ai-batch-detail">
                                        <Show when=move || review_each.get() && cursor.get() == position>
                                            <Effects item=full_item.clone()/>
                                        </Show>
                                        <details hidden=move || review_each.get()>
                                            <summary title=summary_title>
                                                <span class="ai-batch-title">{title}</span>
                                                {icon(Icon::ChevronRight)}
                                                {meta.map(|task| view! {
                                                    <span class="ai-batch-meta"><ListChip list=task.list/>{task.due.map(|d| c.now.with_untracked(|n| n.dates().weekday(d)))}</span>
                                                })}
                                            </summary>
                                            <Effects item=item/>
                                        </details>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            } else {
                items.into_iter().map(|(item, _)| view! { <Effects item/> }).collect_view().into_any()
            }}
            <Show when=move || error.with(Option::is_some)><p class="ai-error" role="alert">{move || error.get()}</p></Show>
            <div class="ai-proposal-actions">
                <button type="button" class="button" on:click=reject>{if batch { "Reject all" } else { "Reject" }}</button>
                <button type="button" class="button primary" disabled=move || !ready() on:click=approve>
                    {move || if batch {
                        let n = selected.with(BTreeSet::len);
                        if n == count { format!("Approve all ({n})") } else { format!("Approve {n}") }
                    } else { "Approve".into() }}
                </button>
            </div>
        </section>
    }
}

#[component]
fn ListChip(list: ListRef) -> impl IntoView {
    let c = ctx();
    let hue = move || {
        c.state.with(|s| {
            s.lists
                .get(&list.id)
                .map(|l| crate::model::swatch(l.color.as_deref(), l.id.as_uuid().as_bytes()))
        })
    };
    let title = list.name.clone();
    view! { <span class="chip ai-list-chip" data-list-hue=hue title=title><span class="chip-label">{list.name}</span></span> }
}

#[component]
fn Effects(item: ProposalItem) -> impl IntoView {
    let c = ctx();
    let date = move |d: Option<_>| {
        d.map_or_else(
            || "No date".to_owned(),
            |d| c.now.with_untracked(|now| now.dates().weekday(d)),
        )
    };
    view! {
        <div class="ai-effects">
            {item.effects.into_iter().map(|effect| match effect {
                Effect::Created { task, priority, status, placement } => {
                    let title = task.title.clone();
                    view! {
                    <p class="ai-proposed-title" title=title>{task.title}</p>
                    <dl>
                        <dt>"List"</dt><dd><ListChip list=task.list/></dd>
                        <dt>"Due"</dt><dd>{date(task.due)}</dd>
                        <dt>"Priority"</dt><dd>{capitalize(priority.name().into())}</dd>
                        <dt>"Status"</dt><dd>{match status { WorkflowStatus::Backlog => "Backlog", WorkflowStatus::Todo => "To do", WorkflowStatus::InProgress => "In progress" }}</dd>
                        <dt>"Position"</dt><dd>{match placement { Placement::First => "Top of the list", Placement::Last => "End of the list", _ => "Relative to another task" }}</dd>
                    </dl>
                }.into_any()
                },
                Effect::Edited { task, changes } => view! {
                    <TaskLink task/>
                    <dl>{changes.into_iter().map(|edit| {
                        let completion = |done| if done { "Completed".to_owned() } else { "Open".to_owned() };
                        let (field, before, after) = match edit {
                            Edit::Title(c) => ("Title", c.old, c.new),
                            Edit::Due(c) => ("Due", date(c.old), date(c.new)),
                            Edit::StartDate(c) => ("Starts", date(c.old), date(c.new)),
                            Edit::Priority(c) => ("Priority", capitalize(c.old.name().into()), capitalize(c.new.name().into())),
                            Edit::Completed(c) => ("Status", completion(c.old), completion(c.new)),
                        };
                        let label = format!("From {before} to {after}");
                        view! { <dt>{field}</dt><dd class="ai-diff" aria-label=label><del>{before}</del><span aria-hidden="true">"→"</span><b>{after}</b></dd> }
                    }).collect_view()}</dl>
                }.into_any(),
                Effect::Deleted { task } => view! { <TaskLink task/><p>"Moves to Trash. You can undo this change from Activity."</p> }.into_any(),
            }).collect_view()}
        </div>
    }
}

#[component]
fn Receipt(id: Uuid, count: usize, tasks: Vec<TaskId>) -> impl IntoView {
    let c = ctx();
    let approval = move || c.ai.approvals.with(|map| map.get(&id).cloned());
    let outcome = Memo::new(move |_| {
        c.sync.track();
        approval().map_or(ApprovalOutcome::Unavailable, |approval| {
            backend::assistant_outcome(&approval)
        })
    });
    leptos::prelude::Effect::new(move |highlighted| {
        let confirmed = matches!(outcome.get(), ApprovalOutcome::Confirmed { .. });
        if confirmed && highlighted != Some(true) {
            c.ai.highlight(id, tasks.clone());
        }
        confirmed || highlighted == Some(true)
    });
    let undoing = RwSignal::new(None);
    let undo_result = Memo::new(move |_| {
        c.sync.track();
        undoing.get().map(backend::undo_result)
    });
    let error = RwSignal::new(None::<String>);
    view! {
        <div class="ai-receipt">
            {mark(&assistant_mark())}
            <span role="status">{move || match (outcome.get(), undo_result.get()) {
                (ApprovalOutcome::Saving, _) => "Saving approved changes…".into(),
                (ApprovalOutcome::Pending, _) => "Approved · waiting for sync".into(),
                (ApprovalOutcome::Confirmed { .. }, Some(None)) => "Undoing approved changes…".into(),
                (ApprovalOutcome::Confirmed { .. }, Some(Some(Ok(())))) => "Approved changes undone".into(),
                (ApprovalOutcome::Confirmed { .. }, Some(Some(Err(e)))) => e.into(),
                (ApprovalOutcome::Confirmed { .. }, None) => format!("Approved · {count} {} confirmed", if count == 1 { "change" } else { "changes" }),
                (ApprovalOutcome::Rejected(error), _) => todo_client::failure::approval_message(&error.code).into(),
                (ApprovalOutcome::Unavailable, _) => "Receipt unavailable · check Activity".into(),
            }}</span>
            <Show when=move || matches!(outcome.get(), ApprovalOutcome::Confirmed { .. }) && matches!(undo_result.get(), None | Some(Some(Err(_))))>
                <button type="button" class="ai-link" disabled=move || c.read_only() on:click=move |_| {
                    if let Some(approval) = untrack(approval) {
                        match backend::undo(&backend::UndoTarget::Live(approval.undo_target().clone())) {
                            Ok(TrackedUndo::Queued(id)) => { undoing.set(Some(id)); error.set(None); c.refresh(); }
                            Ok(_) => error.set(Some("This change is no longer available to undo.".into())),
                            Err(reason) => error.set(Some(reason)),
                        }
                    }
                }>"Undo"</button>
            </Show>
            <Show when=move || error.with(Option::is_some)><span role="alert">{move || error.get()}</span></Show>
        </div>
    }
}
