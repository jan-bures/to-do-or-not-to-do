//! Web presentation of the shared assistant controller. Only explicit review gestures submit.

mod review;
mod settings;

use super::assistant_provider::BrowserProvider;
use super::{Ctx, RoomId, backend, ctx, html, icon, mark, pickers, platform};
use leptos::prelude::*;
use std::{collections::BTreeMap, time::Duration};
use todo_client::assistant::{
    Approval, AssistantView, Composer, Controller, SendIntent, StepKind, StepStatus, TaskRef,
    ToolKind, TurnView, provider::DataClass,
};
use todo_client::web::{BrowserClock, BrowserEntropy};
use todo_core::ids::TaskId;
use todo_design::{Icon, layout::desktop};
use uuid::Uuid;

thread_local! {
    static CHAT: Controller<BrowserProvider, BrowserClock, BrowserEntropy> = Controller::new(BrowserClock, BrowserEntropy);
}

#[derive(Clone, Copy)]
pub(super) struct Ui {
    pub(super) open: RwSignal<bool>,
    pub(super) modal: Memo<bool>,
    pub(super) four_wide: Memo<bool>,
    pub(super) settings: RwSignal<Option<&'static str>>,
    view: RwSignal<AssistantView>,
    profile: StoredValue<settings::Profile>,
    sequence: StoredValue<u64>,
    draft: RwSignal<String>,
    error: RwSignal<Option<String>>,
    return_focus: StoredValue<Option<String>>,
    approvals: RwSignal<BTreeMap<Uuid, Approval>>,
    pub(super) highlights: RwSignal<BTreeMap<TaskId, Uuid>>,
}

impl Ui {
    pub(super) fn new(viewport: RwSignal<i32>, narrow: RwSignal<bool>) -> Self {
        let open = RwSignal::new(false);
        let profile = settings::Profile::load();
        profile.apply(None);
        let view = RwSignal::new(CHAT.with(Controller::view));
        leptos::task::spawn_local(async move {
            let mut seen = CHAT.with(Controller::view_version);
            loop {
                CHAT.with(|chat| chat.changed(seen)).await;
                seen = CHAT.with(Controller::view_version);
                if view.try_set(CHAT.with(Controller::view)).is_some() {
                    break;
                }
            }
        });
        Self {
            open,
            modal: Memo::new(move |_| open.get() && narrow.get()),
            four_wide: Memo::new(move |_| viewport.get() >= desktop::FOUR_PANE_MIN as i32),
            settings: RwSignal::new(None),
            view,
            profile: StoredValue::new(profile),
            sequence: StoredValue::new(0),
            draft: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            return_focus: StoredValue::new(None),
            approvals: RwSignal::new(BTreeMap::new()),
            highlights: RwSignal::new(BTreeMap::new()),
        }
    }

    pub(super) fn sync(self, signed_out: bool) {
        if signed_out {
            self.sign_out();
        } else {
            CHAT.with(|chat| chat.update_snapshot(backend::assistant_snapshot()));
        }
    }

    pub(super) fn sign_out(self) {
        CHAT.with(Controller::sign_out);
        self.draft.set(String::new());
        self.approvals.set(BTreeMap::new());
    }

    fn refresh(self) {
        let next = CHAT.with(Controller::view);
        if self.view.with_untracked(|view| view != &next) {
            self.view.set(next);
        }
    }

    fn active(self) -> bool {
        self.view
            .with_untracked(|view| matches!(view.composer, Composer::Streaming { .. }))
    }

    fn focus_composer(self) {
        request_animation_frame(move || {
            if self.open.get_untracked()
                && self.settings.get_untracked().is_none()
                && !platform::focus("assistant-prompt")
            {
                platform::focus("assistant-connect");
            }
        });
    }

    fn close(self, restore: bool) {
        self.open.set(false);
        if restore {
            let before = self.return_focus.get_value();
            request_animation_frame(move || {
                if !self.open.get_untracked()
                    && !before.is_some_and(|id| platform::focus(&id))
                    && !platform::focus("assistant-trigger")
                {
                    platform::focus("room-title");
                }
            });
        }
    }

    pub(super) fn dismiss(self) {
        self.close(true);
    }

    fn toggle(self, c: Ctx) {
        if self.open.get_untracked() {
            self.close(true);
        } else {
            self.return_focus.set_value(
                pickers::focused()
                    .map(|el| el.id())
                    .filter(|s| !s.is_empty()),
            );
            c.drawer.set(false);
            c.room_actions.close_menu(false);
            self.open.set(true);
            self.focus_composer();
        }
    }

    fn new_chat(self) {
        CHAT.with(Controller::new_chat);
        self.approvals.set(BTreeMap::new());
        self.error.set(None);
        self.draft.set(String::new());
        self.refresh();
        self.focus_composer();
    }

    fn stop(self) {
        CHAT.with(Controller::cancel);
        self.refresh();
        self.focus_composer();
    }

    fn send(self, c: Ctx) {
        let prompt = self.draft.get_untracked();
        if prompt.trim().is_empty() || self.active() || c.sync.with_untracked(|s| s.signed_out) {
            return;
        }
        if self.reload_profile() {
            return;
        }
        self.sync(false);
        let mut sequence = self.sequence.get_value();
        sequence += 1;
        self.sequence.set_value(sequence);
        let context = c.now.get_untracked().eval();
        let result = CHAT.with(|chat| {
            chat.start(SendIntent {
                generation: chat.view().generation,
                sequence,
                prompt,
                context,
            })
        });
        match result {
            Ok(job) => {
                self.error.set(None);
                self.draft.set(String::new());
                self.refresh();
                leptos::task::spawn_local(async move {
                    let _ = job.await;
                });
            }
            Err(error) => self.error.set(Some(error.to_string())),
        }
    }

    fn highlight(self, approval: Uuid, tasks: Vec<TaskId>) {
        self.highlights
            .update(|rows| rows.extend(tasks.into_iter().map(|task| (task, approval))));
        let animation = if platform::reduced_motion() {
            todo_design::motion::COMPLETE_HOLD + todo_design::motion::REDUCED_FADE
        } else {
            todo_design::motion::SYNC_HIGHLIGHT
        };
        set_timeout(
            move || {
                self.highlights
                    .update(|rows| rows.retain(|_, id| *id != approval))
            },
            Duration::from_millis(u64::from(animation)),
        );
    }

    fn follow(self, c: Ctx, id: TaskId) {
        let list = c.state.with_untracked(|s| {
            s.tasks
                .get(&id)
                .filter(|_| s.is_live(id.into()))
                .map(|t| t.list)
        });
        let Some(list) = list else { return };
        if self.modal.get_untracked() {
            self.close(false);
        }
        let visible = c
            .shown
            .with_untracked(|rows| rows.iter().any(|row| row.row.id == id));
        if visible {
            c.select(Some(id));
        } else {
            c.navigate(RoomId::List(list), Some(id));
        }
        request_animation_frame(move || {
            if !c.detail_modal.get_untracked() {
                platform::refocus_row(&format!("row-{id}"));
            } else {
                platform::focus("task-detail");
            }
        });
    }
}

/// Return true when shell navigation must not consume this key.
pub(super) fn key(c: Ctx, ev: &leptos::ev::KeyboardEvent, key: &str) -> bool {
    let ai = c.ai;
    if ai.settings.get_untracked().is_some() {
        return true;
    }
    if key == "j" && (ev.ctrl_key() || ev.meta_key()) && !ev.alt_key() && !ev.shift_key() {
        ev.prevent_default();
        if !ev.repeat() {
            ai.toggle(c);
        }
        return true;
    }
    if !ai.open.get_untracked() {
        return false;
    }
    let modal = ai.modal.get_untracked();
    if modal && key == "tab" {
        platform::contain_tab("assistant-panel", ev);
        return true;
    }
    let inside = pickers::focused()
        .is_some_and(|el| el.closest("#assistant-panel").ok().flatten().is_some());
    if key == "escape" && !ev.repeat() {
        // An open task overlay still has its usual first claim on Escape unless the composer
        // itself is being used (draft or stream).
        ev.prevent_default();
        if (inside || modal) && ai.active() {
            ai.stop();
        } else if inside && ai.draft.with_untracked(|s| !s.is_empty()) {
            ai.draft.set(String::new());
        } else if c.detail_modal.get_untracked() {
            c.select(None);
        } else {
            ai.close(true);
        }
        return true;
    }
    inside || modal
}

#[component]
pub(super) fn Trigger() -> impl IntoView {
    let c = ctx();
    view! {
        <button type="button" class="ai-trigger" id="assistant-trigger" aria-label="Ask AI"
            aria-controls="assistant-panel" aria-keyshortcuts="Control+J Meta+J"
            aria-pressed=move || c.ai.open.get().to_string()
            title="Ask AI (Ctrl+J or Cmd+J)" on:click=move |_| c.ai.toggle(c)>
            {icon(Icon::Sparkles)}<span class="ai-trigger-label">"Ask AI"</span>
            <span class="ai-trigger-count">{move || (!c.ai.open.get() && c.ai.view.with(|v| matches!(v.composer, Composer::Review { .. }))).then_some("1")}</span>
            <kbd class="keycap">"Ctrl J"</kbd>
        </button>
    }
}

#[component]
pub(super) fn Panel() -> impl IntoView {
    let c = ctx();
    let ai = c.ai;
    let feed = NodeRef::<html::Div>::new();
    let stick = StoredValue::new(true);
    let empty = Memo::new(move |_| ai.view.with(|v| v.turns.is_empty()));
    let streaming = Memo::new(move |_| {
        ai.view
            .with(|v| matches!(v.composer, Composer::Streaming { .. }))
    });
    let model = Memo::new(move |_| ai.view.with(|v| v.model.clone()));
    Effect::new(move |_| {
        ai.view.track();
        if ai.open.get() && stick.get_value() {
            request_animation_frame(move || {
                if let Some(el) = feed.get_untracked() {
                    el.set_scroll_top(el.scroll_height());
                }
            });
        }
    });
    view! {
        <section class="assistant-panel" id="assistant-panel" aria-label="AI assistant"
            role=move || ai.modal.get().then_some("dialog") aria-modal=move || ai.modal.get().then_some("true")
            inert=move || !ai.open.get()>
            <div class="ai-handle" aria-hidden="true"></div>
            <header class="ai-header">
                {mark(&assistant_mark())}
                <div class="ai-heading">
                    <h2>{move || ai.view.with(|v| v.turns.first().map_or_else(|| "New chat".into(), |t| t.prompt.lines().next().unwrap_or_default().to_owned()))}</h2>
                    <span>{move || model.with(|m| m.as_ref().map_or("No model chosen", |m| class_label(m.capabilities.data_class)))}</span>
                </div>
                <button type="button" class="icon-button" title="New chat" aria-label="New chat"
                    disabled=move || empty.get() on:click=move |_| ai.new_chat()>{icon(Icon::Plus)}</button>
                <button type="button" class="icon-button" id="assistant-settings" title="AI settings" aria-label="AI settings"
                    aria-haspopup="dialog" on:click=move |_| ai.settings.set(Some("assistant-settings"))>{icon(Icon::Settings)}</button>
                <button type="button" class="icon-button" title="Close assistant" aria-label="Close assistant"
                    on:click=move |_| ai.close(true)>{icon(Icon::X)}</button>
            </header>
            <div class="ai-feed" node_ref=feed on:scroll=move |_| {
                if let Some(el) = feed.get_untracked() { stick.set_value(el.scroll_height() - el.scroll_top() - el.client_height() <= 2); }
            }>
                <div class="ai-feed-content">
                    <Show when=move || empty.get()><Empty/></Show>
                    <For each=move || ai.view.with(|v| v.turns.iter().map(|t| t.id).collect::<Vec<_>>())
                        key=|id| *id children=move |id| view! { <Turn id/> }/>
                </div>
            </div>
            <footer class="ai-footer">
                <div role="status" class="ai-pending" hidden=move || !ai.view.with(|v| matches!(v.composer, Composer::Review { .. }))>
                    {icon(Icon::CircleAlert)}
                    <span>{move || ai.view.with(|v| match v.composer {
                        Composer::Review { selected, total, .. } if total > 1 => format!("{selected} of {total} changes selected"),
                        Composer::Review { .. } => "1 proposal waiting for review".into(),
                        _ => String::new(),
                    })}</span>
                    <button type="button" class="ai-link" on:click=move |_| {
                        if let Some(el) = document().get_element_by_id("assistant-pending-proposal") {
                            el.scroll_into_view();
                            platform::focus("assistant-pending-proposal");
                        }
                    }>"Jump up"</button>
                </div>
                <button type="button" class="ai-privacy" id="assistant-privacy" aria-haspopup="dialog"
                    on:click=move |_| ai.settings.set(Some("assistant-privacy"))>
                    <span class="ai-model">{move || model.with(|m| m.as_ref().map_or_else(|| "Choose a model".into(), |m| format!("{} · {}", class_label(m.capabilities.data_class), m.model)))}</span>
                    <span>{icon(Icon::EyeOff)}{move || {
                        c.state.track();
                        ai.view.track();
                        let (state, allowed) = CHAT.with(Controller::list_scope);
                        state.map(|state| {
                            let hidden = state.lists.keys().filter(|&&id| state.is_live(id.into()) && !allowed.contains(&id)).count();
                            if hidden == 1 { "1 list hidden".into() } else { format!("{hidden} lists hidden") }
                        })
                    }}</span>
                </button>
                <Show when=move || ai.error.with(Option::is_some)><p class="ai-error" role="alert">{move || ai.error.get()}</p></Show>
                <form class="ai-composer" on:submit=move |ev| {
                    ev.prevent_default();
                    stick.set_value(true);
                    ai.send(c);
                }>
                    <textarea id="assistant-prompt" aria-label="Message the assistant" rows="1"
                        disabled=move || model.with(Option::is_none)
                        placeholder=move || ai.view.with(|v| match v.composer {
                            Composer::ChooseModel => "Choose a model to start",
                            Composer::Review { .. } => "Approve above, or reply to adjust…",
                            _ => "Ask about your tasks…",
                        })
                        prop:value=move || ai.draft.get()
                        on:input=move |ev| ai.draft.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                                ev.prevent_default();
                                if !ev.repeat() { stick.set_value(true); ai.send(c); }
                            }
                        }></textarea>
                    <Show when=move || streaming.get() fallback=move || view! {
                        <button type="submit" class="ai-send" aria-label="Send" title="Send (Enter)"
                            disabled=move || model.with(Option::is_none) || ai.draft.with(|s| s.trim().is_empty())>{icon(Icon::ArrowUp)}</button>
                    }>
                        <button type="button" class="ai-send ai-stop" aria-label="Stop" title="Stop"
                            on:click=move |_| ai.stop()>{icon(Icon::Square)}</button>
                    </Show>
                </form>
            </footer>
        </section>
    }
}

fn class_label(class: DataClass) -> &'static str {
    match class {
        DataClass::Local => "This device",
        DataClass::Cloud => "Cloud",
    }
}

#[component]
fn Empty() -> impl IntoView {
    let c = ctx();
    let ai = c.ai;
    let ready = Memo::new(move |_| ai.view.with(|v| v.model.is_some()));
    view! {
        <section class="ai-empty">
            <span class="ai-tile">{icon(Icon::Sparkles)}</span>
            <Show when=move || ready.get() fallback=move || view! {
                <h3>"Connect a model"</h3>
                <p>"Choose an OpenAI-compatible server to start. Lists are hidden until you allow access."</p>
                <button type="button" id="assistant-connect" class="ai-option" aria-haspopup="dialog"
                    on:click=move |_| ai.settings.set(Some("assistant-connect"))>
                    {icon(Icon::Settings)}"Your own server"{icon(Icon::ArrowUpRight)}
                </button>
            }>
                <h3>"What can I help plan?"</h3>
                <p>"Ask about your tasks, or describe changes to make. Every edit or new task waits for your approval first."</p>
                {[ (Icon::Calendar, "What’s due this week?", "What’s due this week?"),
                    (Icon::List, "Summarize open tasks", "Summarize my open tasks."),
                    (Icon::Flag, "Review my priorities", "Help me review my priorities."),
                    (Icon::Plus, "Turn a rough note into tasks…", "Turn this note into tasks: ") ]
                    .map(|(glyph, label, prompt)| view! {
                        <button type="button" class="ai-option" on:click=move |_| {
                            ai.draft.set(prompt.into());
                            ai.focus_composer();
                        }>{icon(glyph)}<span>{label}</span>{icon(Icon::ArrowUpRight)}</button>
                    })}
            </Show>
        </section>
    }
}

fn step_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Generate => "Waiting for the model",
        StepKind::Tool(ToolKind::Lists) => "Checked permitted lists",
        StepKind::Tool(ToolKind::Search) => "Searched visible tasks",
        StepKind::Tool(ToolKind::Task) => "Read task details",
        StepKind::Tool(ToolKind::Create) => "Prepared a new task",
        StepKind::Tool(ToolKind::Update) => "Prepared task edits",
        StepKind::Tool(ToolKind::Complete) => "Prepared a completion change",
    }
}

#[component]
fn Turn(id: Uuid) -> impl IntoView {
    let c = ctx();
    let ai = c.ai;
    let field = move |pick: fn(&TurnView) -> String| {
        move || {
            ai.view.with(|v| {
                v.turns
                    .iter()
                    .find(|t| t.id == id)
                    .map(pick)
                    .unwrap_or_default()
            })
        }
    };
    let logs = Memo::new(move |_| {
        ai.view.with(|v| {
            v.turns
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.summary, t.steps.clone()))
        })
    });
    let active = Memo::new(move |_| {
        ai.view
            .with(|v| matches!(v.composer, Composer::Streaming { turn } if turn == id))
    });
    let links = Memo::new(move |_| {
        ai.view.with(|v| {
            v.turns
                .iter()
                .find(|t| t.id == id)
                .map_or_else(Vec::new, |turn| {
                    turn.task_links
                        .iter()
                        .filter(|link| {
                            turn.proposal.as_ref().is_none_or(|card| {
                                card.status != todo_client::assistant::ProposalStatus::Pending
                                    || !card
                                        .preview
                                        .items
                                        .iter()
                                        .flat_map(|item| &item.effects)
                                        .any(|effect| match effect {
                                            todo_client::assistant::Effect::Edited {
                                                task, ..
                                            }
                                            | todo_client::assistant::Effect::Deleted { task } => {
                                                task.id == link.id
                                            }
                                            _ => false,
                                        })
                            })
                        })
                        .cloned()
                        .collect()
                })
        })
    });
    let error = Memo::new(move |_| {
        ai.view
            .with(|v| v.turns.iter().find(|t| t.id == id).and_then(|t| t.error))
    });
    view! {
        <article class="ai-turn">
            <p class="ai-prompt">{field(|t| t.prompt.clone())}</p>
            <Show when=move || active.get()>
                <p class="ai-step-live" role="status">{move || logs.with(|r| r.as_ref().and_then(|(_, s)| s.last()).map(|s| step_label(s.kind)))}</p>
            </Show>
            <Show when=move || !active.get() && logs.with(|l| l.as_ref().is_some_and(|(_, steps)| !steps.is_empty()))>
                <details class="ai-steps">
                    <summary>{move || logs.with(|l| l.as_ref().map(|(s, _)| format!("Checked {} lists · Read {} tasks · {} hidden", s.lists_checked, s.tasks_read, s.private_lists)))}</summary>
                    <ul>{move || logs.with(|l| l.as_ref().map(|(_, steps)| steps.iter().map(|s| {
                        let elapsed = match s.status { StepStatus::Done { elapsed, .. } => Some(format!("{} ms", elapsed.as_millis())), _ => None };
                        view! { <li>{step_label(s.kind)}<span>{elapsed}</span></li> }
                    }).collect_view()))}</ul>
                </details>
            </Show>
            <p class="ai-prose" aria-busy=move || active.get().to_string()>{field(|t| t.prose.clone())}</p>
            <For each=move || links.get() key=|task| task.id children=move |task| view! { <TaskLink task/> }/>
            <review::Proposal turn=id/>
            {move || error.get().map(|error| view! {
                <div class="ai-error" role="status"><p>{match error {
                    todo_client::assistant::AssistantError::Provider(todo_client::assistant::provider::ProviderError::Unavailable) =>
                        "Couldn’t reach your model. Check the address, connection, and the provider’s browser access (CORS).".to_owned(),
                    _ => format!("{}.", capitalize(error.to_string())),
                }}</p>
                <div class="ai-error-actions">
                    <button type="button" class="ai-link" on:click=move |_| {
                        if let Some(prompt) = ai.view.with_untracked(|v| v.turns.iter().find(|t| t.id == id).map(|t| t.prompt.clone())) {
                            ai.draft.set(prompt);
                            ai.send(c);
                            ai.focus_composer();
                        }
                    }>"Retry"</button>
                    <button type="button" class="ai-link" on:click=move |_| ai.settings.set(Some("assistant-settings"))>"AI settings"</button>
                </div></div>
            })}
        </article>
    }
}

fn capitalize(s: String) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

#[component]
fn TaskLink(task: TaskRef) -> impl IntoView {
    let c = ctx();
    let id = task.id;
    let title = task.title.clone();
    view! {
        <button type="button" class="ai-task-link" title=title
            on:click=move |_| c.ai.follow(c, id)>
            <span>{task.title}</span>
            {task.due.map(|d| view! { <span class="ai-task-date">{c.now.with_untracked(|now| now.dates().weekday(d))}</span> })}
            {icon(Icon::ArrowUpRight)}
        </button>
    }
}

fn assistant_mark() -> crate::model::AgentMark {
    crate::model::AgentMark {
        letter: "A".into(),
        key: Some("assistant"),
        name: "Assistant".into(),
    }
}

pub(super) fn is_profile_key(key: Option<&str>) -> bool {
    key.is_none_or(|key| key == settings::KEY)
}

pub(super) use settings::Settings;
