//! The main window's root: the pairing screen until this computer is paired, then the shell on the live
//! session. Signing out goes back to pairing.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AsyncWindowContext, Context, Entity, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window, div, prelude::*,
};

use super::failed::{FailedEvent, FailedView};
use super::pairing::{PairingEvent, PairingView};
use super::popouts::Popouts;
use super::quick_add::QuickAddView;
use super::shell::{ShellEvent, ShellView};
use crate::config::{self, Pairing, Paths};
use crate::model::{RoomKey, Smart, UiState};
use crate::session::{self, OpenError, Pending, Recovered, Runtime, Session, SignedOut};
use crate::store::{LiveStore, Store};
use crate::theme::Theme;

/// What startup loads before the first frame, off the UI thread.
#[allow(missing_debug_implementations)] // Holds the session.
pub struct Startup {
    pub rt: Runtime,
    pub paths: Paths,
    pub opened: Opened,
    /// The server to prefill when pairing.
    pub last_server: Option<String>,
}

/// What opening the saved pairing gave.
#[allow(missing_debug_implementations)] // Holds the session.
pub enum Opened {
    /// Not paired: show pairing.
    NotPaired,
    /// The saved sign-in couldn't be read (or can't be used): pair again. The old data stays.
    PairAgain(SharedString),
    /// Paired, but the cache couldn't be opened now. Nothing was removed; trying again may work.
    /// `pending`: an unfinished recovery or start fresh, which decides what the screen offers.
    Failed {
        why: SharedString,
        pending: Option<Pending>,
    },
    /// The paired session, opened on its cache (not syncing yet), and what was recovered if the cache
    /// was damaged.
    Session(Box<Session>, Option<Recovered>),
}

/// Loads the saved pairing and opens its cache. Runs on a thread of its own while GPUI starts, so a warm
/// start has its data before the first frame.
pub fn load() -> anyhow::Result<Startup> {
    let rt = session::runtime()?;
    let paths = Paths::for_user()?;
    let opened = open(&rt, &paths);
    Ok(Startup {
        last_server: config::last_server(&paths),
        rt,
        paths,
        opened,
    })
}

/// Opens the saved pairing. Never deletes a cache because it failed to open: a damaged one is set aside
/// with its unsent changes carried over, anything else is reported and left alone. Only data of pairings
/// that were signed out is cleaned up here.
fn open(rt: &Runtime, paths: &Paths) -> Opened {
    match config::load(paths) {
        Ok(Some(pairing)) => {
            config::sweep_signed_out(paths, Some(&pairing.config.pairing));
            match Session::open_or_recover(rt.clone(), paths, &pairing) {
                Ok((s, recovered)) => Opened::Session(Box::new(s), recovered),
                Err(OpenError::Config(why)) => {
                    tracing::warn!("the saved sign-in can't be used: {why}");
                    Opened::PairAgain(
                        format!("The saved sign-in can't be used ({why}). Connect again.").into(),
                    )
                }
                Err(e) => {
                    tracing::warn!("the local data couldn't be opened: {e}");
                    Opened::Failed {
                        why: format!("{}.", capitalized(&e.to_string())).into(),
                        pending: Session::pending(paths, &pairing),
                    }
                }
            }
        }
        Ok(None) => {
            config::sweep_signed_out(paths, None);
            Opened::NotPaired
        }
        Err(e) => {
            tracing::warn!("the saved pairing couldn't be read: {e:#}");
            Opened::PairAgain("The saved sign-in couldn't be read. Connect again.".into())
        }
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// What the shell says after a damaged cache was rebuilt.
fn recovered_notice(r: &Recovered) -> String {
    let kept = match r.kept {
        0 => String::new(),
        1 => " 1 unsent change was kept.".into(),
        n => format!(" {n} unsent changes were kept."),
    };
    let lost = match r.lost {
        0 => String::new(),
        n => format!(" {n} couldn't be read back."),
    };
    format!(
        "The local data was damaged, so it was set aside and is being loaded again from the server.{kept}{lost}"
    )
}

/// What the shell says after starting fresh.
pub const STARTED_FRESH: &str = "Started fresh from the server. Unsent changes in the damaged data \
                                 weren't recovered; a copy is kept.";

/// Opens the saved pairing on an empty cache, the damaged data set aside (`Session::start_fresh`).
/// On failure, what to show: why, and what is still pending.
fn start_fresh(
    rt: &Runtime,
    paths: &Paths,
) -> Result<(Session, session::StartedFresh), (SharedString, Option<Pending>)> {
    let pairing = match config::load(paths) {
        Ok(Some(p)) => p,
        Ok(None) => return Err(("The saved sign-in is gone. Restart the app.".into(), None)),
        Err(e) => {
            return Err((
                format!("The saved sign-in couldn't be read: {e:#}.").into(),
                None,
            ));
        }
    };
    Session::start_fresh(rt.clone(), paths, &pairing).map_err(|e| {
        tracing::warn!("starting fresh failed: {e}");
        (
            format!("Starting fresh didn't work: {e}.").into(),
            Session::pending(paths, &pairing),
        )
    })
}

enum Screen {
    /// While `RootView::new` runs, and while signing out lets go of the shell.
    Starting,
    Pairing(Entity<PairingView>),
    Shell(Entity<ShellView>),
    /// The paired data couldn't be opened; nothing was removed.
    Failed(Entity<FailedView>),
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct RootView {
    rt: Runtime,
    paths: Paths,
    theme: Theme,
    screen: Screen,
    quick: Option<WeakEntity<QuickAddView>>,
    focus: FocusHandle,
    signing_out: bool,
    restore_popouts: bool,
    popouts: Option<Popouts>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(
        startup: Startup,
        theme: Theme,
        restore_popouts: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Startup {
            rt,
            paths,
            opened,
            last_server,
        } = startup;
        let mut this = Self {
            rt,
            paths,
            theme,
            screen: Screen::Starting,
            quick: None,
            focus: cx.focus_handle(),
            signing_out: false,
            restore_popouts,
            popouts: None,
            _subscriptions: Vec::new(),
        };
        this.show(opened, last_server, window, cx);
        this
    }

    fn show(
        &mut self,
        opened: Opened,
        last_server: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match opened {
            Opened::Session(s, recovered) => {
                self.show_shell(*s, window, cx);
                if let (Some(r), Screen::Shell(shell)) = (recovered, &self.screen) {
                    let text = recovered_notice(&r);
                    shell.update(cx, |s, cx| s.show_notice(text, false, cx));
                }
            }
            Opened::NotPaired => self.show_pairing(last_server, None, window, cx),
            Opened::PairAgain(why) => self.show_pairing(last_server, Some(why), window, cx),
            Opened::Failed { why, pending } => self.show_failed(why, pending, window, cx),
        }
    }

    fn show_failed(
        &mut self,
        why: SharedString,
        pending: Option<Pending>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme;
        let view = cx.new(|cx| FailedView::new(theme, why, pending, cx));
        let sub =
            cx.subscribe_in(
                &view,
                window,
                |this, _, event: &FailedEvent, window, cx| match event {
                    FailedEvent::Retry => this.retry(window, cx),
                    FailedEvent::StartFresh => this.start_fresh(window, cx),
                },
            );
        self._subscriptions = vec![sub];
        let focus = view.focus_handle(cx);
        window.focus(&focus, cx);
        self.screen = Screen::Failed(view);
        cx.notify();
    }

    /// Starting fresh was confirmed: set the damaged data aside off the UI thread, then show the shell on
    /// an empty cache and say what was given up, with a way to the kept copy.
    fn start_fresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (rt, paths) = (self.rt.clone(), self.paths.clone());
        self._subscriptions.clear();
        self.screen = Screen::Starting;
        cx.notify();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async move { start_fresh(&rt, &paths) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((s, fresh)) => {
                    this.show_shell(s, window, cx);
                    if let Screen::Shell(shell) = &this.screen {
                        shell.update(cx, |s, cx| {
                            s.show_folder_notice(STARTED_FRESH, fresh.kept, cx);
                        });
                    }
                }
                Err((why, pending)) => this.show_failed(why, pending, window, cx),
            });
        })
        .detach();
    }

    /// Opens the saved pairing again (after a failure), off the UI thread.
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (rt, paths) = (self.rt.clone(), self.paths.clone());
        self.screen = Screen::Starting;
        cx.notify();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let opened = cx
                .background_executor()
                .spawn({
                    let (rt, paths) = (rt.clone(), paths.clone());
                    async move { open(&rt, &paths) }
                })
                .await;
            let last = config::last_server(&paths);
            let _ = this.update_in(cx, |this, window, cx| this.show(opened, last, window, cx));
        })
        .detach();
    }

    fn show_pairing(
        &mut self,
        server: Option<String>,
        error: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (rt, paths, theme) = (self.rt.clone(), self.paths.clone(), self.theme);
        let view = cx.new(|cx| PairingView::new(rt, paths, theme, server, error, window, cx));
        let sub = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &PairingEvent, window, cx| {
                let PairingEvent::Paired(pairing) = event;
                this.paired(pairing, window, cx);
            },
        );
        self._subscriptions = vec![sub];
        self.screen = Screen::Pairing(view);
        self.push_progress(cx);
        cx.notify();
    }

    fn paired(&mut self, pairing: &Pairing, window: &mut Window, cx: &mut Context<Self>) {
        // A new pairing has a directory of its own, so nothing of an earlier one is picked up.
        match Session::open(self.rt.clone(), &self.paths, pairing) {
            Ok(s) => self.show_shell(s, window, cx),
            Err(e) => {
                tracing::error!("opening the session failed: {e}");
                let server = Some(pairing.config.server.clone());
                let why = format!("Paired, but the local data couldn't be created ({e}).");
                self.show_pairing(server, Some(why.into()), window, cx);
            }
        }
    }

    fn show_shell(&mut self, session: Session, window: &mut Window, cx: &mut Context<Self>) {
        let data = session.data().clone();
        let prefs = data.load_ui();
        let ui = UiState {
            collapsed_collections: prefs.collapsed_collections,
            board_done: prefs.board_done,
            panes: prefs.panes,
            ..UiState::default()
        };
        session.start();
        let store = Rc::new(Store::Live(LiveStore::new(session)));
        let theme = self.theme;
        let view = cx.new(|cx| ShellView::new(store, ui, theme, None, window, cx));
        // Reminder toasts, also while the window is hidden in the tray.
        view.update(cx, |s, cx| s.start_reminders(window, cx));
        let events =
            cx.subscribe_in(
                &view,
                window,
                |this, _, event: &ShellEvent, window, cx| match event {
                    ShellEvent::SignOut => this.sign_out(window, cx),
                    ShellEvent::PopOut(room) => {
                        if !this.signing_out
                            && let Some(popouts) = &mut this.popouts
                        {
                            popouts.open(*room, true, cx);
                        }
                    }
                },
            );
        let changes = cx.observe(&view, |this, _, cx| {
            this.push_progress(cx);
            this.reconcile_popouts(cx);
        });
        if self.restore_popouts {
            self.popouts = Some(Popouts::new(data, view.downgrade(), window, theme));
            // In startup the main HWND is still being built. Open sibling windows after that.
            let root = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = root.update(cx, Self::reconcile_popouts);
            });
        }
        self._subscriptions = vec![events, changes];
        let focus = view.focus_handle(cx);
        window.focus(&focus, cx);
        self.screen = Screen::Shell(view);
        self.push_progress(cx);
        cx.notify();
    }

    fn reconcile_popouts(&mut self, cx: &mut Context<Self>) {
        if !self.signing_out
            && let Some(popouts) = &mut self.popouts
        {
            popouts.reconcile(cx);
        }
    }

    /// Signs out (`session::sign_out`): stops syncing, removes the saved sign-in, then the pairing's
    /// data, and only then shows pairing. If the sign-in can't be removed, the shell stays, syncing
    /// again, and says so.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Shell(shell) = &self.screen else {
            return;
        };
        if std::mem::replace(&mut self.signing_out, true) {
            return;
        }
        let store = shell.read(cx).store().clone();
        let Some(session) = store.session() else {
            self.signing_out = false;
            return;
        };
        // The other windows must let go of the session before its cache is removed.
        if let Some(mut popouts) = self.popouts.take() {
            popouts.close_all(cx);
        }
        // The pairing's reminder toasts go with it.
        shell.update(cx, |s, _| s.stop_reminders());
        let stopped = session.shutdown();
        let server = session.server().to_string();
        let data = session.data().clone();
        drop(store);
        shell.update(cx, |s, cx| s.show_notice("Signing out…", false, cx));
        let paths = self.paths.clone();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let executor = cx.background_executor().clone();
            let weak = this.clone();
            let mut release_cx = cx.clone();
            let release = move || {
                // Drops the shell, and with it the session and its handle on the cache.
                let _ = weak.update(&mut release_cx, |this, cx| {
                    this._subscriptions.clear();
                    this.screen = Screen::Starting;
                    cx.notify();
                });
            };
            let result = session::sign_out(stopped, &paths, &data, release, || {
                executor.timer(Duration::from_millis(100))
            })
            .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.signing_out = false;
                match result {
                    Ok(SignedOut::Clean) => this.show_pairing(Some(server), None, window, cx),
                    Ok(SignedOut::DataLeft(_)) => {
                        let note = "Signed out. Some local data was still in use; it's removed \
                                    the next time the app starts.";
                        this.show_pairing(Some(server), Some(note.into()), window, cx);
                    }
                    Err(e) => this.still_signed_in(&e, window, cx),
                }
            });
        })
        .detach();
    }

    /// Signing out stopped before anything was removed: open the session again and say why.
    fn still_signed_in(
        &mut self,
        why: &session::StillSignedIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::error!("signing out failed: {why}");
        let text = why.notice();
        let reopened = config::load(&self.paths)
            .ok()
            .flatten()
            .ok_or_else(|| OpenError::Config("the saved sign-in is gone".into()))
            .and_then(|p| Session::open(self.rt.clone(), &self.paths, &p));
        match reopened {
            Ok(s) => {
                self.show_shell(s, window, cx);
                if let Screen::Shell(shell) = &self.screen {
                    shell.update(cx, |s, cx| s.show_notice(text, true, cx));
                }
            }
            Err(e) => self.show_failed(format!("{text} ({e})").into(), None, window, cx),
        }
    }

    /// The app is quitting: save the list windows for next time and remove the reminder toasts.
    pub fn before_quit(&mut self, cx: &mut Context<Self>) {
        if let Some(mut popouts) = self.popouts.take() {
            popouts.close_all(cx);
        }
        if let Screen::Shell(shell) = &self.screen {
            shell.update(cx, |s, _| s.stop_reminders());
        }
    }

    /// The clock was changed or the computer woke up: the shell looks at its reminders again.
    pub fn clock_changed(&mut self, cx: &mut Context<Self>) {
        if let Screen::Shell(shell) = &self.screen {
            shell.update(cx, |s, cx| s.clock_changed(cx));
        }
    }

    /// The quick-add window to keep in step with My Day.
    pub fn set_quick_add(&mut self, quick: WeakEntity<QuickAddView>, cx: &mut Context<Self>) {
        self.quick = Some(quick);
        self.push_progress(cx);
    }

    fn push_progress(&self, cx: &mut Context<Self>) {
        let progress = self.my_day_progress(cx);
        if let Some(q) = self.quick.as_ref().and_then(WeakEntity::upgrade) {
            q.update(cx, |q, cx| q.set_progress(progress, cx));
        }
    }

    /// My Day's progress, for the quick-add window's sun.
    pub fn my_day_progress(&self, cx: &App) -> (u32, u32) {
        match &self.screen {
            Screen::Shell(s) => s.read(cx).my_day_progress(),
            Screen::Pairing(_) | Screen::Starting | Screen::Failed(_) => (0, 0),
        }
    }

    /// Adds a task typed into the global quick-add to My Day. Returns false when there is nowhere to add
    /// it yet (not paired).
    pub fn quick_add(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        match &self.screen {
            Screen::Shell(s) => s.update(cx, |s, cx| {
                s.add_task(text, RoomKey::Smart(Smart::MyDay), cx)
            }),
            Screen::Pairing(_) | Screen::Starting | Screen::Failed(_) => false,
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.screen {
            Screen::Pairing(p) => p.focus_handle(cx),
            Screen::Shell(s) => s.focus_handle(cx),
            Screen::Failed(f) => f.focus_handle(cx),
            Screen::Starting => self.focus.clone(),
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let screen = match &self.screen {
            Screen::Pairing(p) => p.clone().into_any_element(),
            Screen::Shell(s) => s.clone().into_any_element(),
            Screen::Failed(f) => f.clone().into_any_element(),
            Screen::Starting => div().into_any_element(),
        };
        div().size_full().child(screen)
    }
}
