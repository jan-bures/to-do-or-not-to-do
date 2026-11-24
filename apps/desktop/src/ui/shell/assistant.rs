//! GPUI glue for the native assistant. Network work is started from explicit gestures only.

mod cloud;
mod fixtures;
#[cfg(test)]
mod tests;
mod views;

use super::*;
use crate::{
    assistant_profile::Profile,
    chatgpt::{self, DesktopProvider},
    session,
    ui::text_area::TextArea,
};
use futures::future::{AbortHandle, Abortable};
use std::collections::{BTreeMap, BTreeSet};
use todo_client::{
    CommandId, TrackedUndo,
    assistant::{
        Approval, ApprovalOutcome, AssistantError, AssistantView, Composer, Controller,
        ProposalCard, ProposalStatus, SendIntent, TurnView,
        local::{Destination, MAX_RETRY_AFTER, Support, valid_model},
        provider::{Model, Provider, ProviderError},
    },
    native::{OsEntropy, TokioClock},
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Default)]
struct Review {
    turn: Option<Uuid>,
    expanded: BTreeSet<usize>,
    each: Option<usize>,
    zero: bool,
    show_all: bool,
}

impl Review {
    fn begin(&mut self, turn: Uuid) {
        if self.turn != Some(turn) {
            *self = Self {
                turn: Some(turn),
                ..Self::default()
            };
        }
    }

    fn indices(&self, card: &ProposalCard) -> BTreeSet<usize> {
        if self.zero {
            BTreeSet::new()
        } else {
            card.preview.items.iter().map(|item| item.index).collect()
        }
    }
}

struct Receipt {
    approval: Approval,
    undo: Option<CommandId>,
    tasks: Vec<TaskId>,
    confirmed: bool,
}

#[derive(Default)]
enum Check {
    #[default]
    NotTested,
    Running(&'static str),
    Catalog {
        count: usize,
        elapsed: Duration,
    },
    NoCatalog,
    Capable(Support),
    Unreachable(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConnectionHealth {
    #[default]
    Unknown,
    Testing,
    Connected,
    Unreachable,
}

fn unreachable(error: ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Unavailable
            | ProviderError::Timeout
            | ProviderError::Tls
            | ProviderError::TailnetDisconnected
    )
}

const NOT_PRIVATE: &str = "For a non-loopback model, use the private relay on your To-do server.";

struct Setup {
    draft: Profile,
    saved_file: bool,
    check: Check,
    models: Vec<Model>,
    pick_model: bool,
    abort: Option<AbortHandle>,
    revision: Uuid,
    access_version: Option<Uuid>,
}

impl Setup {
    fn invalidate(&mut self) {
        self.revision = Uuid::new_v4();
        if let Some(pending) = self.abort.take() {
            pending.abort();
        }
        self.check = Check::NotTested;
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        if let Some(pending) = self.abort.take() {
            pending.abort();
        }
    }
}

pub(super) struct Ai {
    pub(super) open: bool,
    stopped: bool,
    pub(super) detail_over_room: bool,
    pub(super) viewport: f32,
    pub(super) composer: Entity<TextArea>,
    url: Entity<TextInput>,
    model: Entity<TextInput>,
    key: Entity<TextInput>,
    profile: Profile,
    controller: Controller<DesktopProvider, TokioClock, OsEntropy>,
    cloud: cloud::Cloud,
    settings: Option<Setup>,
    close_to: Option<FocusHandle>,
    panel: FocusHandle,
    dialog: FocusHandle,
    focus: RefCell<BTreeMap<String, FocusHandle>>,
    feed: kit::ScrollPane,
    settings_scroll: kit::ScrollPane,
    model_scroll: kit::ScrollPane,
    composer_scroll: kit::ScrollPane,
    consent_scroll: kit::ScrollPane,
    sequence: u64,
    notice: Option<String>,
    step_details: BTreeSet<Uuid>,
    expanded_task_links: BTreeSet<Uuid>,
    review: Review,
    receipts: BTreeMap<CommandId, Receipt>,
    flash: HashMap<TaskId, Duration>,
    fixture: Option<AssistantView>,
    rate_until: Option<Instant>,
    seen_error: Option<Uuid>,
    connection: ConnectionHealth,
}

impl Ai {
    pub(super) fn new(
        store: &Store,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<ShellView>,
    ) -> Self {
        let composer = cx.new(|cx| {
            let mut a = TextArea::new(
                "Ask anything, or propose a change…",
                quick_add_colors(p),
                cx,
            );
            a.chat = true;
            a.focus_handle = a.focus_handle.tab_stop(true);
            a
        });
        cx.subscribe_in(
            &composer,
            window,
            |this, _, event, window, cx| match event {
                InputEvent::Submit => this.send_ai_input(window, cx),
                InputEvent::Cancel => this.escape_ai(window, cx),
                InputEvent::Changed => cx.notify(),
            },
        )
        .detach();
        let mut field = |placeholder, secret| {
            cx.new(|cx| {
                let mut i = TextInput::new(placeholder, quick_add_colors(p), cx);
                i.secret = secret;
                i.focus_handle = i.focus_handle.tab_stop(true);
                i
            })
        };
        let url = field("Enter a loopback URL", false);
        let model = field("Choose or enter a model ID", false);
        let key = field("Optional bearer key", true);
        for (entity, field) in [
            (&url, Field::Url),
            (&model, Field::Model),
            (&key, Field::Key),
        ] {
            cx.subscribe_in(entity, window, move |this, _, e, window, cx| match e {
                InputEvent::Changed => this.ai_setting_changed(field, cx),
                InputEvent::Cancel => this.escape_ai(window, cx),
                InputEvent::Submit => {} // Never connect just by editing a field.
            })
            .detach();
        }
        let (profile, notice) = match store.session().map(|s| Profile::load(s.data())).transpose() {
            Ok(profile) => (profile.unwrap_or_default(), None),
            Err(_) => (
                Profile::default(),
                Some("Couldn't read the protected assistant settings.".into()),
            ),
        };
        let controller = Controller::new(TokioClock, OsEntropy);
        let cloud = cloud::Cloud::load(store);
        controller.configure(cloud.selection(&profile));
        if let Some(session) = store.session() {
            controller.update_snapshot(session.client().assistant_snapshot());
        }
        Self {
            open: false,
            stopped: false,
            detail_over_room: false,
            viewport: f32::from(window.viewport_size().width),
            composer,
            url,
            model,
            key,
            profile,
            controller,
            cloud,
            settings: None,
            close_to: None,
            panel: cx.focus_handle(),
            dialog: cx.focus_handle(),
            focus: RefCell::default(),
            feed: kit::ScrollPane::default(),
            settings_scroll: kit::ScrollPane::default(),
            model_scroll: kit::ScrollPane::default(),
            composer_scroll: kit::ScrollPane::default(),
            consent_scroll: kit::ScrollPane::default(),
            sequence: 0,
            notice,
            step_details: BTreeSet::new(),
            expanded_task_links: BTreeSet::new(),
            review: Review::default(),
            receipts: BTreeMap::new(),
            flash: HashMap::new(),
            fixture: None,
            rate_until: None,
            seen_error: None,
            connection: ConnectionHealth::Unknown,
        }
    }

    fn destination(&self) -> Option<Destination> {
        self.profile.endpoint().map(|endpoint| endpoint.destination)
    }

    fn same_destination(&self, draft: &Profile) -> bool {
        draft.cloud == self.profile.cloud
            && draft.url == self.profile.url
            && draft.key == self.profile.key
            && draft.private_relay == self.profile.private_relay
            && draft.chatgpt_account == self.profile.chatgpt_account
    }

    fn connection_health(&self) -> ConnectionHealth {
        if self.settings.as_ref().is_some_and(|setup| {
            self.same_destination(&setup.draft) && matches!(setup.check, Check::Running(_))
        }) {
            ConnectionHealth::Testing
        } else {
            self.connection
        }
    }

    fn blocked(&self) -> bool {
        !self.profile.uses_relay() && !self.profile.url.is_empty() && self.destination().is_none()
    }

    fn route_label(&self) -> &'static str {
        if let Some(route) = self.profile.hosted() {
            route.label()
        } else {
            self.destination()
                .map_or("Assistant off", Destination::label)
        }
    }

    fn view(&self) -> AssistantView {
        self.fixture
            .clone()
            .unwrap_or_else(|| self.controller.view())
    }

    pub(super) fn modal(&self) -> bool {
        self.settings.is_some() || self.cloud.consent.is_some()
    }

    pub(super) fn focus_in(&self, window: &Window, cx: &App) -> bool {
        self.panel.contains_focused(window, cx) || self.dialog.contains_focused(window, cx)
    }

    fn focus_handle(&self, key: &str, cx: &App) -> FocusHandle {
        self.focus
            .borrow_mut()
            .entry(key.into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    pub(super) fn set_colors(&self, p: &Palette, cx: &mut Context<ShellView>) {
        let colors = quick_add_colors(p);
        self.composer.update(cx, |i, cx| {
            i.colors = colors;
            cx.notify();
        });
        for field in [&self.url, &self.model, &self.key] {
            field.update(cx, |i, cx| {
                i.colors = colors;
                cx.notify();
            });
        }
    }

    fn clear_chat_state(&mut self) {
        self.notice = None;
        self.review = Review::default();
        self.receipts.clear();
        self.flash.clear();
        self.step_details.clear();
        self.expanded_task_links.clear();
        self.feed.reset_to_top();
    }
}

#[derive(Clone, Copy)]
enum Field {
    Url,
    Model,
    Key,
}

fn route_problem(profile: &Profile, error: ProviderError) -> String {
    if profile.cloud {
        cloud::cloud_problem(error)
    } else if profile.private_relay {
        error.private_relay_message().into()
    } else {
        problem(error)
    }
}

fn problem(error: ProviderError) -> String {
    match error {
        ProviderError::Unavailable | ProviderError::Timeout => {
            "Local provider unavailable. Your server may be asleep or off your private network."
                .into()
        }
        ProviderError::Tls => {
            "Couldn't verify your server's HTTPS certificate. Check the address and certificate."
                .into()
        }
        ProviderError::TailnetDisconnected => {
            "Tailscale isn't connected on this PC. Connect it and retry.".into()
        }
        ProviderError::NotPrivate => NOT_PRIVATE.into(),
        ProviderError::Unauthorized => {
            "API key required or invalid. Check the key in AI settings.".into()
        }
        ProviderError::RateLimited { .. } => {
            "Your server's usage limit was reached. No request will be retried automatically."
                .into()
        }
        ProviderError::ModelMissing => {
            "The selected model isn't available. Choose a model in AI settings.".into()
        }
        ProviderError::Unsupported => "Your server or model doesn't support this operation.".into(),
        ProviderError::InvalidResponse | ProviderError::Incomplete => {
            "The reply was incomplete or unreadable. No changes were prepared.".into()
        }
        ProviderError::Limit => {
            "The response exceeded the assistant's size or output limit.".into()
        }
    }
}

impl ShellView {
    pub(super) fn ai_docked(&self) -> bool {
        self.ai.open && self.width == Width::Wide
    }
    pub(super) fn ai_shares_detail(&self) -> bool {
        self.ai.viewport >= desktop::FOUR_PANE_MIN
    }

    pub(super) fn ai_destination(&self) -> Option<SharedString> {
        self.ai.open.then(|| {
            let route = self.ai.route_label();
            self.ai.view().model.as_ref().map_or_else(
                || route.into(),
                |model| format!("{route} · {}", model::labels::one_line(&model.model)).into(),
            )
        })
    }

    pub(super) fn refresh_ai(&mut self, cx: &mut Context<Self>) {
        if self.ui.compact || self.ai.stopped {
            return;
        }
        let Some(session) = self.store.session() else {
            return;
        };
        if session.client().revocation().is_revoked() {
            self.stop_assistant(cx);
            return;
        }
        let now = self.now();
        for receipt in self.ai.receipts.values_mut().filter(|r| !r.confirmed) {
            if matches!(
                session.client().approval_outcome(&receipt.approval),
                ApprovalOutcome::Confirmed { .. }
            ) {
                receipt.confirmed = true;
                self.ai
                    .flash
                    .extend(receipt.tasks.iter().map(|&id| (id, now)));
            }
        }
        let before = self.ai.controller.view().generation;
        self.ai
            .controller
            .update_snapshot(session.client().assistant_snapshot());
        if self.ai.controller.view().generation != before {
            self.ai.clear_chat_state();
            self.ai.composer_scroll.reset_to_top();
            self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        }
    }

    pub(super) fn drive_ai_flash(&mut self, window: &mut Window) {
        let reduced = self.pace().reduced;
        self.ai
            .flash
            .retain(|_, at| self.frame.saturating_sub(*at) < flash_span(reduced));
        if self.animate && !self.ai.flash.is_empty() {
            window.request_animation_frame();
        }
    }

    pub(super) fn ai_flash(&self, id: TaskId) -> Option<f32> {
        self.ai.flash.get(&id).map(|&at| {
            let reduced = self.pace().reduced;
            let hold = if reduced {
                Duration::from_millis(u64::from(todo_design::motion::COMPLETE_HOLD))
            } else {
                Duration::ZERO
            };
            Track::new(
                1.,
                0.,
                at + hold,
                flash_span(reduced) - hold,
                todo_design::motion::EXIT,
            )
            .at(self.frame)
        })
    }

    pub(crate) fn stop_assistant(&mut self, cx: &mut Context<Self>) {
        self.ai.stopped = true;
        self.ai.controller.sign_out();
        self.ai.cloud.close();
        self.ai.profile = Profile::default();
        self.ai.open = false;
        self.settle_detail();
        self.ai.clear_chat_state();
        self.ai.settings = None;
        self.ai.composer_scroll.reset_to_top();
        self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        self.ai.key.update(cx, |i, cx| i.set_text("", cx));
    }

    pub(super) fn shell_ai_key(
        &mut self,
        e: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if e.is_held || e.keystroke.modifiers.modified() {
            return;
        }
        if e.keystroke.key == "escape"
            && self.ai.open
            && !self.ai.focus_in(window, cx)
            && window.focused(cx).is_some_and(|f| f == self.list_focus)
        {
            self.escape_ai(window, cx);
            cx.stop_propagation();
        } else if navigation_tab(e)
            && !e.keystroke.modifiers.shift
            && self.ai.open
            && self.splitters.focus(Pane::Ai).is_focused(window)
        {
            self.focus_ai(window, cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn toggle_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pairing_panel.is_some() || self.ai.modal() || self.ai.stopped {
            return;
        }
        if self.ai.open {
            self.close_ai(window, cx);
            return;
        }
        if !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return;
        }
        if self.menu.is_some() {
            self.close_menu(false, window, cx);
        }
        self.ai.open = true;
        self.refresh_ai_relay(cx);
        self.ai.detail_over_room = false;
        self.settle_detail();
        self.focus_ai(window, cx);
        cx.notify();
    }

    fn focus_ai(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ai.blocked() || self.ai.view().composer == Composer::ChooseModel {
            window.focus(&self.ai.panel, cx);
        } else {
            window.focus(&self.ai.composer.read(cx).focus_handle.clone(), cx);
        }
    }

    fn close_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ai.open = false;
        self.ai.detail_over_room = false;
        self.settle_detail();
        window.focus(&self.list_focus, cx);
        cx.notify();
    }

    pub(super) fn escape_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ai.cloud.consent.take().is_some() {
            self.focus_ai(window, cx);
        } else if let Some(setup) = self.ai.settings.as_mut() {
            if setup.pick_model {
                setup.pick_model = false;
                cx.notify();
            } else {
                self.close_ai_settings(window, cx);
            }
        } else if matches!(self.ai.view().composer, Composer::Streaming { .. }) {
            self.ai.controller.cancel();
        } else if !self.ai.composer.read(cx).content.is_empty() {
            self.ai
                .composer
                .update(cx, |area, cx| area.set_text("", cx));
        } else if self.ai.detail_over_room && !self.ai_shares_detail() {
            self.select_task(None, cx);
            self.focus_ai(window, cx);
        } else {
            self.close_ai(window, cx);
        }
        cx.notify();
    }

    fn send_ai_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ai.composer.read(cx).content.to_string();
        self.send_ai(text, cx);
        if self.ai.cloud.consent.is_some() {
            let focus = self.ai.focus_handle("assistant-chatgpt-consent-cancel", cx);
            window.focus(&focus, cx);
        }
    }

    fn send_ai(&mut self, text: String, cx: &mut Context<Self>) {
        if self.ai.profile.uses_relay()
            && !text.trim().is_empty()
            && !self.ai.modal()
            && !self.ai.cloud.sending
            && !self.ai.stopped
            && !matches!(
                self.ai.controller.view().composer,
                Composer::Streaming { .. }
            )
            && let (Some(session), Some(relay)) =
                (self.store.session(), self.ai.cloud.relay.clone())
        {
            self.ai.cloud.sending = true;
            let route = self.ai.profile.hosted();
            let model = self.ai.profile.model.clone();
            let current_draft = self.ai.composer.read(cx).content.to_string();
            let work = session::spawn(
                &session.assistant_runtime(),
                async move { relay.status().await },
            );
            cx.spawn(async move |this, cx| {
                let result = work.await.unwrap_or(Err(ProviderError::Unavailable));
                let _ = this.update(cx, |this, cx| {
                    this.ai.cloud.sending = false;
                    if this.ai.stopped || !this.ai.open || this.ai.modal() || this.ai.profile.hosted() != route
                        || this.ai.profile.model != model || this.ai.composer.read(cx).content.as_ref() != current_draft
                    { return; }
                    match result {
                        Ok(status) => {
                            this.apply_ai_relay_status(status, cx);
                            if this.ai.controller.view().model.is_some() { this.send_ai_refreshed(text, cx); }
                            else { this.ai.notice = Some("This model or account is no longer connected. Check AI settings.".into()); }
                        }
                        Err(err) => this.ai.notice = Some(route_problem(&this.ai.profile, err)),
                    }
                    cx.notify();
                });
            }).detach();
            return;
        }
        if !self.ai.profile.uses_relay() {
            self.send_ai_refreshed(text, cx);
        }
    }

    fn send_ai_refreshed(&mut self, text: String, cx: &mut Context<Self>) {
        if text.trim().is_empty()
            || self.ai.blocked()
            || self.ai.modal()
            || (self.ai.profile.cloud && self.ai.cloud.usage_paused)
            || self.ai.rate_until.is_some_and(|end| end > Instant::now())
        {
            return;
        }
        self.refresh_ai(cx);
        let Some(session) = self.store.session() else {
            return;
        };
        let rt = session.assistant_runtime();
        let view = self.ai.controller.view();
        if matches!(
            view.composer,
            Composer::ChooseModel | Composer::Streaming { .. }
        ) {
            return;
        }
        if self.require_cloud_consent(&text, cx) {
            return;
        }
        if self.ai.review.zero
            && let Some(proposal) = self.ai.controller.pending()
        {
            let preview = proposal.preview();
            let _ = self.ai.controller.discard(preview.id, preview.digest);
        }
        let clear_draft = self.ai.composer.read(cx).content.as_ref() == text;
        self.ai.sequence = self.ai.sequence.saturating_add(1);
        let job = match self.ai.controller.start(SendIntent {
            generation: view.generation,
            sequence: self.ai.sequence,
            prompt: text,
            context: self.store.now().eval(),
        }) {
            Ok(job) => job,
            Err(error) => {
                self.ai.notice = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.ai.notice = None;
        self.ai.review = Review::default();
        let active_view = self.ai.controller.view();
        self.ai.receipts.retain(|id, _| {
            active_view
                .turns
                .iter()
                .any(|t| t.proposal.as_ref().is_some_and(|p| p.preview.id == *id))
        });
        self.ai
            .step_details
            .retain(|id| active_view.turns.iter().any(|t| t.id == *id));
        self.ai
            .expanded_task_links
            .retain(|id| active_view.turns.iter().any(|t| t.id == *id));
        if clear_draft {
            self.ai.composer_scroll.reset_to_top();
            self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        }
        self.ai.feed.handle.scroll_to_bottom();
        let work = session::spawn(&rt, job);
        let mut version = self.ai.controller.view_version();
        cx.spawn(async move |this, cx| {
            let _ = work.await;
            let _ = this.update(cx, |view, cx| {
                view.observe_ai_error(cx);
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(changed)) = this.update(cx, |shell, _| {
                    matches!(
                        shell.ai.controller.view().composer,
                        Composer::Streaming { .. }
                    )
                    .then(|| shell.ai.controller.changed(version))
                }) else {
                    break;
                };
                changed.await;
                cx.background_executor()
                    .timer(Duration::from_millis(
                        u64::from(todo_design::motion::HOVER) / 3,
                    ))
                    .await;
                if this
                    .update(cx, |shell, cx| {
                        version = shell.ai.controller.view_version();
                        let near_bottom = -shell.ai.feed.handle.offset().y + px(space::S10)
                            >= shell.ai.feed.handle.max_offset().y;
                        if near_bottom {
                            shell.ai.feed.handle.scroll_to_bottom();
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn observe_ai_error(&mut self, cx: &mut Context<Self>) {
        let view = self.ai.view();
        let Some(turn) = view.turns.last() else {
            return;
        };
        if self.ai.seen_error == Some(turn.id) {
            return;
        }
        if matches!(view.composer, Composer::Streaming { .. }) {
            return;
        }
        match turn.error {
            Some(AssistantError::Provider(error)) if unreachable(error) => {
                self.ai.connection = ConnectionHealth::Unreachable
            }
            None => self.ai.connection = ConnectionHealth::Connected,
            _ => {}
        }
        if let Some(AssistantError::Provider(error)) = turn.error {
            self.ai.seen_error = Some(turn.id);
            if self.ai.composer.read(cx).content.is_empty() {
                self.ai
                    .composer
                    .update(cx, |a, cx| a.set_text(turn.prompt.clone(), cx));
            }
            if let ProviderError::RateLimited { retry_after } = error {
                if self.ai.profile.cloud {
                    self.ai.cloud.usage_paused = true;
                } else {
                    self.ai_rate_timer(retry_after, cx);
                }
            }
        } else if turn
            .proposal
            .as_ref()
            .is_some_and(|p| p.status == ProposalStatus::Pending)
        {
            self.ai.review.begin(turn.id);
        }
    }

    fn ai_rate_timer(&mut self, retry: Option<Duration>, cx: &mut Context<Self>) {
        let Some(delay) = retry.filter(|d| !d.is_zero()) else {
            return;
        };
        self.ai.rate_until = Instant::now().checked_add(delay.min(MAX_RETRY_AFTER));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(u64::from(
                        todo_design::motion::PAIRING_TICK,
                    )))
                    .await;
                if !this
                    .update(cx, |shell, cx| {
                        cx.notify();
                        shell.ai.rate_until.is_some_and(|end| end > Instant::now())
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn new_ai_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ai.controller.new_chat();
        self.ai.fixture = None;
        self.ai.clear_chat_state();
        self.ai.composer_scroll.reset_to_top();
        self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        self.focus_ai(window, cx);
        cx.notify();
    }

    fn open_ai_task(&mut self, task: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.store.view();
        let Some(t) = state
            .tasks
            .get(&task)
            .filter(|_| state.is_live(task.into()))
        else {
            self.ai.notice = Some("That task is no longer available.".into());
            cx.notify();
            return;
        };
        if !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return;
        }
        let done = t.completed_at.is_some();
        if self.width == Width::Narrow {
            self.close_ai(window, cx);
        }
        self.filter.update(cx, |i, cx| i.reset(window, cx));
        self.ui.filter.clear();
        self.select_room(
            if done {
                RoomKey::Smart(model::Smart::Completed)
            } else {
                RoomKey::List(t.list)
            },
            cx,
        );
        if self.on_board() || self.on_timeline() {
            self.switch_view(todo_core::model::ListView::List, window, cx);
        }
        self.select_task(Some(task), cx);
        window.focus(&self.list_focus, cx);
    }

    fn approve_ai(&mut self, id: Uuid, digest: [u8; 32], cx: &mut Context<Self>) {
        if self.ai.review.zero || self.ai.modal() {
            return;
        }
        let Some(session) = self.store.session() else {
            return;
        };
        let tasks = self
            .ai
            .controller
            .pending()
            .as_ref()
            .map(|proposal| {
                proposal
                    .preview()
                    .items
                    .iter()
                    .flat_map(|item| &item.effects)
                    .map(|effect| {
                        use todo_client::assistant::Effect;
                        match effect {
                            Effect::Created { task, .. }
                            | Effect::Edited { task, .. }
                            | Effect::Deleted { task } => task.id,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        match self.ai.controller.approve(session.client(), id, digest) {
            Ok(approval) => {
                self.ai.receipts.insert(
                    id,
                    Receipt {
                        approval,
                        undo: None,
                        tasks,
                        confirmed: false,
                    },
                );
                self.ai.notice = None;
                self.refresh(cx);
            }
            Err(error) => self.ai.notice = Some(error.to_string()),
        }
        cx.notify();
    }

    fn decline_ai(&mut self, id: Uuid, digest: [u8; 32], cx: &mut Context<Self>) {
        if let Err(error) = self.ai.controller.discard(id, digest) {
            self.ai.notice = Some(error.to_string());
        } else {
            self.ai.notice = None;
            self.ai.review = Review::default();
        }
        cx.notify();
    }

    fn select_ai_item(
        &mut self,
        turn: Uuid,
        id: Uuid,
        digest: [u8; 32],
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let view = self.ai.view();
        let Some(card) = view
            .turns
            .last()
            .filter(|t| t.id == turn)
            .and_then(|t| t.proposal.as_ref())
        else {
            return;
        };
        if card.status != ProposalStatus::Pending
            || card.preview.id != id
            || card.preview.digest != digest
        {
            return;
        }
        self.ai.review.begin(turn);
        let mut selected = self.ai.review.indices(card);
        if !selected.remove(&index) {
            selected.insert(index);
        }
        if selected.is_empty() {
            self.ai.review.zero = true;
            self.ai.notice = None;
        } else {
            match self.ai.controller.select_items(id, digest, &selected) {
                Ok(()) => {
                    self.ai.review.zero = false;
                    self.ai.notice = None;
                }
                Err(error) => self.ai.notice = Some(error.to_string()),
            }
        }
        cx.notify();
    }

    fn undo_ai(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let (Some(receipt), Some(session)) = (self.ai.receipts.get_mut(&id), self.store.session())
        else {
            return;
        };
        let sync = session.client();
        if !matches!(
            sync.approval_outcome(&receipt.approval),
            ApprovalOutcome::Confirmed { .. }
        ) {
            return;
        }
        if receipt
            .undo
            .is_some_and(|id| !matches!(sync.undo_result(id), Some(Err(_))))
        {
            return;
        }
        match sync.compensate_tracked(receipt.approval.undo_target()) {
            Ok(TrackedUndo::Queued(undo)) => {
                receipt.undo = Some(undo);
                self.ai.notice = None;
            }
            _ => {
                self.ai.notice =
                    Some("Couldn't undo this change. Check Activity for its current status.".into())
            }
        }
        cx.notify();
    }

    pub(super) fn open_ai_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pairing_panel.is_some() || self.ai.modal() || self.ai.stopped {
            return;
        }
        if self.menu.is_some() {
            self.close_menu(false, window, cx);
        }
        self.ai.settings_scroll.reset_to_top();
        self.ai.model_scroll.reset_to_top();
        let p = &self.ai.profile;
        self.ai
            .url
            .update(cx, |i, cx| i.set_text(p.url.clone(), cx));
        self.ai.model.update(cx, |i, cx| {
            let model = if valid_model(&p.model) {
                model::labels::one_line(&p.model)
            } else {
                String::new()
            };
            i.set_text(model, cx);
        });
        self.ai.key.update(cx, |i, cx| {
            i.set_text("", cx);
            i.placeholder = if p.key.is_empty() {
                "Optional bearer key"
            } else {
                "Key saved · enter to replace"
            }
            .into();
        });
        self.ai.settings = Some(Setup {
            saved_file: self
                .store
                .session()
                .is_some_and(|session| Profile::exists(session.data())),
            draft: Profile {
                url: p.url.clone(),
                model: p.model.clone(),
                cloud: p.cloud,
                private_relay: p.private_relay,
                chatgpt_account: p.chatgpt_account,
                access: if p.uses_relay() {
                    self.ai
                        .cloud
                        .status
                        .as_ref()
                        .map_or_else(Default::default, |s| s.access.clone())
                } else {
                    p.access.clone()
                },
                support: p.support,
                key: p.key.clone(),
            },
            check: p.support.map_or(Check::NotTested, Check::Capable),
            models: Vec::new(),
            pick_model: false,
            abort: None,
            revision: Uuid::new_v4(),
            access_version: self.ai.cloud.status.as_ref().map(|s| s.access_version),
        });
        self.ai.close_to = window.focused(cx);
        if p.uses_relay() {
            window.focus(&self.ai.dialog, cx);
        } else {
            window.focus(&self.ai.url.read(cx).focus_handle.clone(), cx);
        }
        self.refresh_ai_relay(cx);
        cx.notify();
    }

    fn close_ai_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ai.settings = None;
        self.ai.cloud.code = None;
        self.ai.cloud.signed_in = None;
        self.ai.key.update(cx, |i, cx| i.set_text("", cx));
        if let Some(handle) = self.ai.close_to.take() {
            window.focus(&handle, cx);
        } else if self.ai.open {
            self.focus_ai(window, cx);
        } else {
            window.focus(&self.account_focus, cx);
        }
        cx.notify();
    }

    fn pick_local_model(&mut self, id: &str, cx: &mut Context<Self>) {
        self.ai.model.update(cx, |input, cx| input.set_text(id, cx));
        self.ai_setting_changed(Field::Model, cx);
        if let Some(setup) = &mut self.ai.settings {
            setup.pick_model = false;
        }
    }

    fn ai_setting_changed(&mut self, field: Field, cx: &mut Context<Self>) {
        let Some(setup) = self.ai.settings.as_mut() else {
            return;
        };
        setup.invalidate();
        setup.draft.support = None;
        match field {
            Field::Url => {
                let url = self.ai.url.read(cx).content.to_string();
                // A credential saved for the old destination must never be tested against a new URL.
                setup.draft.change_url(url);
                self.ai.key.update(cx, |i, cx| {
                    i.set_text("", cx);
                    i.placeholder = "Optional bearer key".into();
                });
                setup.models.clear();
                setup.pick_model = false;
            }
            Field::Model => setup.draft.model = self.ai.model.read(cx).content.to_string(),
            Field::Key => {
                setup.draft.key = Zeroizing::new(self.ai.key.read(cx).content.to_string());
                setup.models.clear();
            }
        }
        cx.notify();
    }

    fn test_ai(&mut self, capability: bool, cx: &mut Context<Self>) {
        if self.ai.rate_until.is_some_and(|end| end > Instant::now()) {
            return;
        }
        let (Some(setup), Some(session)) = (self.ai.settings.as_mut(), self.store.session()) else {
            return;
        };
        if matches!(setup.check, Check::Running(_)) || session.client().revocation().is_revoked() {
            return;
        }
        let provider = match setup.draft.provider() {
            Ok(provider) => provider,
            Err(error) => {
                setup.check = Check::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        if capability && !valid_model(&setup.draft.model) {
            setup.check = Check::Failed("Choose a model to test first.".into());
            cx.notify();
            return;
        }
        setup.invalidate();
        let revision = setup.revision;
        let model = setup.draft.model.clone();
        let (abort, registration) = AbortHandle::new_pair();
        setup.abort = Some(abort);
        setup.check = Check::Running(if capability {
            "Testing with a sample prompt…"
        } else {
            "Checking connection…"
        });
        let work = session::spawn(
            &session.assistant_runtime(),
            Abortable::new(
                async move {
                    if capability {
                        provider.probe(&model).await.map(Tested::Support)
                    } else {
                        let start = Instant::now();
                        provider
                            .list_models()
                            .await
                            .map(|models| Tested::Catalog(models, start.elapsed()))
                    }
                },
                registration,
            ),
        );
        cx.spawn(async move |this, cx| {
            let result = match work.await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => return,
                Err(_) => Err(ProviderError::Unavailable),
            };
            let _ = this.update(cx, |this, cx| {
                let Some(setup) = this
                    .ai
                    .settings
                    .as_mut()
                    .filter(|s| s.revision == revision && s.abort.is_some())
                else {
                    return;
                };
                setup.abort = None;
                if setup.draft.url == this.ai.profile.url && setup.draft.key == this.ai.profile.key
                {
                    match result {
                        Ok(_) | Err(ProviderError::Unsupported) => {
                            this.ai.connection = ConnectionHealth::Connected
                        }
                        Err(error) if unreachable(error) => {
                            this.ai.connection = ConnectionHealth::Unreachable
                        }
                        _ => {}
                    }
                }
                match result {
                    Ok(Tested::Support(support)) => {
                        setup.draft.support = Some(support);
                        setup.check = Check::Capable(support);
                    }
                    Ok(Tested::Catalog(models, elapsed)) => {
                        let missing = valid_model(&setup.draft.model)
                            && !models.iter().any(|m| m.id == setup.draft.model);
                        setup.check = if missing {
                            Check::Failed(problem(ProviderError::ModelMissing))
                        } else {
                            Check::Catalog {
                                count: models.len(),
                                elapsed,
                            }
                        };
                        this.ai.model_scroll.reset_to_top();
                        setup.models = models;
                        setup.pick_model = !setup.models.is_empty();
                    }
                    Err(ProviderError::Unsupported) if !capability => {
                        setup.check = Check::NoCatalog
                    }
                    Err(error) => {
                        setup.check = if unreachable(error) {
                            Check::Unreachable(problem(error))
                        } else {
                            Check::Failed(problem(error))
                        };
                        if let ProviderError::RateLimited { retry_after } = error {
                            this.ai_rate_timer(retry_after, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_ai_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_relay_access(window, cx) {
            return;
        }
        let (Some(setup), Some(session)) = (self.ai.settings.as_mut(), self.store.session()) else {
            return;
        };
        if !setup.draft.model.is_empty() && !valid_model(&setup.draft.model) {
            setup.check = Check::Failed("Choose a valid model.".into());
            cx.notify();
            return;
        }
        if !setup.draft.uses_relay()
            && (!setup.draft.url.is_empty() || !setup.draft.model.is_empty())
            && let Err(error) = setup.draft.normalize()
        {
            setup.check = Check::Failed(error.to_string());
            cx.notify();
            return;
        }
        if setup.draft.save(session.data()).is_err() {
            setup.check = Check::Failed("Couldn't save the protected assistant settings.".into());
            cx.notify();
            return;
        }
        let next = std::mem::take(&mut setup.draft);
        let old = &self.ai.profile;
        let changing = old.cloud != next.cloud
            || old.url != next.url
            || old.model != next.model
            || old.key != next.key
            || old.private_relay != next.private_relay
            || old.chatgpt_account != next.chatgpt_account
            || old.support != next.support;
        let endpoint_changed = old.cloud != next.cloud
            || old.url != next.url
            || old.key != next.key
            || old.private_relay != next.private_relay
            || old.chatgpt_account != next.chatgpt_account;
        let generation = self.ai.controller.view().generation;
        if changing {
            self.ai.controller.configure(self.ai.cloud.selection(&next));
        } else if !next.uses_relay() && old.access != next.access {
            self.ai.controller.set_scope(next.access.clone());
        }
        if endpoint_changed {
            self.ai.rate_until = None;
            self.ai.connection = match setup.check {
                Check::Catalog { .. } | Check::Capable(_) | Check::NoCatalog => {
                    ConnectionHealth::Connected
                }
                Check::Unreachable(_) => ConnectionHealth::Unreachable,
                _ => ConnectionHealth::Unknown,
            };
        }
        if self.ai.controller.view().generation != generation {
            self.ai.clear_chat_state();
            self.ai.composer_scroll.reset_to_top();
            self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        }
        self.ai.profile = next;
        self.close_ai_settings(window, cx);
    }

    fn forget_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.store.session() else {
            return;
        };
        // Cancel all in-flight work and release plaintext immediately, even if disk cleanup fails.
        self.ai.controller.configure(None);
        self.ai.clear_chat_state();
        self.ai.profile = Profile::default();
        self.ai.rate_until = None;
        self.ai.connection = ConnectionHealth::Unknown;
        self.ai.composer_scroll.reset_to_top();
        self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        self.ai.key.update(cx, |i, cx| {
            i.set_text("", cx);
            i.placeholder = "Optional bearer key".into();
        });
        self.ai.url.update(cx, |i, cx| i.set_text("", cx));
        self.ai.model.update(cx, |i, cx| i.set_text("", cx));
        if let Some(setup) = &mut self.ai.settings {
            setup.invalidate();
            setup.draft = Profile::default();
            setup.models.clear();
            setup.pick_model = false;
        }
        if Profile::forget(session.data()).is_err() {
            if let Some(setup) = &mut self.ai.settings {
                setup.saved_file = true;
                setup.check = Check::Failed(
                    "Couldn't remove all protected assistant settings. Try Forget again.".into(),
                );
            }
            cx.notify();
        } else {
            self.close_ai_settings(window, cx);
        }
    }
}

fn flash_span(reduced: bool) -> Duration {
    use todo_design::motion::{COMPLETE_HOLD, REDUCED_FADE, SYNC_HIGHLIGHT};
    Duration::from_millis(u64::from(if reduced {
        COMPLETE_HOLD + REDUCED_FADE
    } else {
        SYNC_HIGHLIGHT
    }))
}

enum Tested {
    Catalog(Vec<Model>, Duration),
    Support(Support),
}
