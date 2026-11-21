//! The main window's root: the pairing screen until this computer is paired, then the shell on the live
//! session. Signing out goes back to pairing.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, Entity, FocusHandle, Focusable, SharedString, Subscription, WeakEntity,
    Window, div, prelude::*,
};

use super::pairing::{PairingEvent, PairingView};
use super::quick_add::QuickAddView;
use super::shell::{ShellEvent, ShellView};
use crate::config::{self, Pairing, Paths};
use crate::model::{RoomKey, Smart, UiState};
use crate::session::{self, Runtime, Session};
use crate::store::{LiveStore, Store};
use crate::theme::Theme;

/// What startup loads before the first frame, off the UI thread.
#[allow(missing_debug_implementations)] // Holds the session.
pub struct Startup {
    pub rt: Runtime,
    pub paths: Paths,
    /// The paired session, opened on its cache (not syncing yet).
    pub session: Option<Session>,
    /// The server to prefill when pairing.
    pub last_server: Option<String>,
    /// Why pairing is shown although a pairing was saved.
    pub error: Option<SharedString>,
}

/// Loads the saved pairing and opens its cache. Runs on a thread of its own while GPUI starts, so a warm
/// start has its data before the first frame.
pub fn load() -> anyhow::Result<Startup> {
    let rt = session::runtime()?;
    let paths = Paths::for_user()?;
    let (session, error) = match config::load(&paths) {
        Ok(Some(pairing)) => match open(&rt, &paths, &pairing) {
            Ok(s) => (Some(s), None),
            Err(e) => (
                None,
                Some(format!("The local data couldn't be opened ({e}). Connect again.").into()),
            ),
        },
        Ok(None) => (None, None),
        Err(e) => {
            tracing::warn!("the saved pairing couldn't be read: {e:#}");
            (
                None,
                Some("The saved sign-in couldn't be read. Connect again.".into()),
            )
        }
    };
    Ok(Startup {
        last_server: config::last_server(&paths),
        rt,
        paths,
        session,
        error,
    })
}

/// Opens the session on its cache. A cache that can't be read is dropped and rebuilt from the server.
fn open(rt: &Runtime, paths: &Paths, pairing: &Pairing) -> anyhow::Result<Session> {
    match Session::open(rt.clone(), &paths.cache(), pairing) {
        Ok(s) => Ok(s),
        Err(e) => {
            tracing::warn!("the cache couldn't be opened, starting afresh: {e:#}");
            config::forget_cache(paths)?;
            Session::open(rt.clone(), &paths.cache(), pairing)
        }
    }
}

enum Screen {
    /// Only while `RootView::new` runs.
    Starting,
    Pairing(Entity<PairingView>),
    Shell(Entity<ShellView>),
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct RootView {
    rt: Runtime,
    paths: Paths,
    theme: Theme,
    screen: Screen,
    quick: Option<WeakEntity<QuickAddView>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(
        startup: Startup,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Startup {
            rt,
            paths,
            session,
            last_server,
            error,
        } = startup;
        let mut this = Self {
            rt,
            paths,
            theme,
            screen: Screen::Starting,
            quick: None,
            focus: cx.focus_handle(),
            _subscriptions: Vec::new(),
        };
        match session {
            Some(s) => this.show_shell(s, window, cx),
            None => this.show_pairing(last_server, error, window, cx),
        }
        this
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
        // A cache left from an earlier pairing belongs to that one.
        if let Err(e) = config::forget_cache(&self.paths) {
            tracing::warn!("an old cache couldn't be removed: {e:#}");
        }
        match Session::open(self.rt.clone(), &self.paths.cache(), pairing) {
            Ok(s) => self.show_shell(s, window, cx),
            Err(e) => {
                tracing::error!("opening the session failed: {e:#}");
                let server = Some(pairing.config.server.clone());
                let why = format!("Paired, but the local data couldn't be created ({e}).");
                self.show_pairing(server, Some(why.into()), window, cx);
            }
        }
    }

    fn show_shell(&mut self, session: Session, window: &mut Window, cx: &mut Context<Self>) {
        session.start();
        let store = Rc::new(Store::Live(LiveStore::new(session)));
        let theme = self.theme;
        let view = cx.new(|cx| ShellView::new(store, UiState::default(), theme, None, window, cx));
        let events =
            cx.subscribe_in(
                &view,
                window,
                |this, _, event: &ShellEvent, window, cx| match event {
                    ShellEvent::SignOut => this.sign_out(window, cx),
                },
            );
        let changes = cx.observe(&view, |this, _, cx| this.push_progress(cx));
        self._subscriptions = vec![events, changes];
        let focus = view.focus_handle(cx);
        window.focus(&focus, cx);
        self.screen = Screen::Shell(view);
        self.push_progress(cx);
        cx.notify();
    }

    /// Signs out: forgets the token and the config at once, stops syncing, then removes the cache once
    /// the sync loops have let go of it.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Shell(shell) = &self.screen else {
            return;
        };
        let store = shell.read(cx).store().clone();
        let server = store.session().map(|s| s.server().to_string());
        let stopped = store.session().map(Session::shutdown);
        drop(store);
        if let Err(e) = config::forget_pairing(&self.paths) {
            tracing::error!("removing the saved sign-in failed: {e:#}");
        }
        self.show_pairing(server, None, window, cx);
        let paths = self.paths.clone();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            if let Some(stopped) = stopped {
                stopped.await;
            }
            // The shell's last handle on the cache goes when its view is released, just after this.
            for attempt in 0..50 {
                match config::forget_cache(&paths) {
                    Ok(()) => return,
                    Err(e) if attempt == 49 => {
                        tracing::warn!("the cache couldn't be removed: {e:#}");
                    }
                    Err(_) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                    }
                }
            }
        })
        .detach();
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
            Screen::Pairing(_) | Screen::Starting => (0, 0),
        }
    }

    /// Adds a task typed into the global quick-add to My Day. Returns false when there is nowhere to add
    /// it yet (not paired).
    pub fn quick_add(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        match &self.screen {
            Screen::Shell(s) => s.update(cx, |s, cx| {
                s.add_task(text, RoomKey::Smart(Smart::MyDay), cx)
            }),
            Screen::Pairing(_) | Screen::Starting => false,
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.screen {
            Screen::Pairing(p) => p.focus_handle(cx),
            Screen::Shell(s) => s.focus_handle(cx),
            Screen::Starting => self.focus.clone(),
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let screen = match &self.screen {
            Screen::Pairing(p) => p.clone().into_any_element(),
            Screen::Shell(s) => s.clone().into_any_element(),
            Screen::Starting => div().into_any_element(),
        };
        div().size_full().child(screen)
    }
}
