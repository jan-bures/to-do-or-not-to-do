//! Independent normal HWNDs on the main shell's store. Only the main shell owns the live session's
//! reminders and account UI; each list window owns its selection, scroll, composer and animation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Focusable, TitlebarOptions, WeakEntity,
    Window, WindowHandle, WindowOptions, px, size,
};
use todo_client::Readiness;

use crate::config::DataDir;
use crate::model::UiState;
use crate::model::popouts::{MIN_SIZE, Preference, Preferences, Room, SIZE};
use crate::platform::win;
use crate::theme::{Material, Theme};

use super::shell::ShellView;

type Handle = WindowHandle<ShellView>;

/// Shared by the list windows for one pairing, so moves and closes never overwrite another room's
/// preferences. The geometry comes from the native normal window, not its maximized or iconic rect.
pub(super) struct Settings {
    data: DataDir,
    prefs: Preferences,
    dirty: bool,
}

impl Settings {
    pub fn update(&mut self, room: Room, change: impl FnOnce(&mut Preference)) -> bool {
        let mut prefs = self.prefs.get(room);
        change(&mut prefs);
        let changed = self.prefs.set(room, prefs);
        self.dirty |= changed;
        changed
    }

    pub fn sample(&mut self, room: Room, hwnd: isize) -> bool {
        let Some(place) = win::list_window_placement(hwnd) else {
            return false;
        };
        self.update(room, |prefs| prefs.place = Some(place))
    }

    pub fn save(&mut self) {
        if self.dirty {
            match self.data.save_popouts(&self.prefs) {
                Ok(()) => self.dirty = false,
                Err(error) => tracing::warn!(%error, "couldn't save list window settings"),
            }
        }
    }
}

pub(super) struct Popouts {
    windows: HashMap<Room, Handle>,
    settings: Rc<RefCell<Settings>>,
    main: WeakEntity<ShellView>,
    main_window: AnyWindowHandle,
    theme: Theme,
}

impl Popouts {
    pub fn new(data: DataDir, main: WeakEntity<ShellView>, window: &Window, theme: Theme) -> Self {
        let prefs = data.load_popouts();
        Self {
            windows: HashMap::new(),
            settings: Rc::new(RefCell::new(Settings {
                data,
                prefs,
                dirty: false,
            })),
            main,
            main_window: window.window_handle(),
            theme,
        }
    }

    /// Restore without taking the foreground (including `--background`). An absent list may be on
    /// a later page of a recovering cache; don't forget it until the replica has caught up.
    pub fn reconcile(&mut self, cx: &mut App) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let store = main.read(cx).store();
        let (state, ready) = store.view_and_readiness();
        let rooms: Vec<_> = self.settings.borrow().prefs.open_rooms().collect();
        self.windows.retain(|_, handle| handle.read(cx).is_ok());
        for room in rooms {
            if room.available(&state) {
                if !self.windows.contains_key(&room) {
                    self.open(room, false, cx);
                }
            } else if ready == Readiness::CaughtUp {
                if let Some(handle) = self.windows.remove(&room) {
                    let _ = handle.update(cx, |_, window, _| {
                        self.settings
                            .borrow_mut()
                            .sample(room, win::hwnd_of(window));
                        window.remove_window();
                    });
                }
                let mut settings = self.settings.borrow_mut();
                settings.update(room, |prefs| prefs.open = false);
                settings.save();
            }
        }
    }

    /// One window per room. Choosing Pop out again restores and raises the existing one.
    pub fn open(&mut self, room: Room, focus: bool, cx: &mut App) {
        if let Some(hwnd) = self
            .windows
            .get(&room)
            .and_then(|handle| handle.update(cx, |_, window, _| win::hwnd_of(window)).ok())
        {
            if focus {
                cx.spawn(async move |_| win::show_and_focus(hwnd)).detach();
            }
            return;
        }
        self.windows.remove(&room);
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let store = main.read(cx).store().clone();
        if !room.available(&store.view()) || store.sync().unauthorized {
            return;
        }
        let saved = self.settings.borrow().prefs.get(room);
        let theme = self.theme;
        let settings = self.settings.clone();
        let main = self.main.clone();
        let main_window = self.main_window;
        let handle = match cx.open_window(window_options(cx), |window, cx| {
            let ui = UiState {
                room: room.key(),
                compact: true,
                ..UiState::default()
            };
            let view = cx.new(|cx| {
                let mut view = ShellView::new(store, ui, theme, None, window, cx);
                view.wire_popout(
                    room,
                    saved.pinned,
                    settings.clone(),
                    (main_window, main),
                    window,
                    cx,
                );
                view
            });
            window.focus(&view.focus_handle(cx), cx);
            view
        }) {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%error, "couldn't open a list window");
                return;
            }
        };
        let cascade = self.windows.len();
        self.windows.insert(room, handle);
        let Ok(hwnd) = handle.update(cx, |_, window, _| win::hwnd_of(window)) else {
            return;
        };
        // SetWindowPos (and ShowWindow) can synchronously deliver GPUI's resize/move callbacks.
        // Do the placement outside the App borrow, and before making the HWND visible.
        cx.spawn(async move |cx: &mut AsyncApp| {
            if !cx.update(|cx| handle.read(cx).is_ok()) {
                return;
            }
            if let Err(error) = win::position_list_window(hwnd, saved.place, cascade) {
                tracing::warn!(%error, "couldn't position a list window");
            }
            let pinned = saved.pinned
                && match win::set_topmost(hwnd, true) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(%error, "couldn't pin a list window");
                        false
                    }
                };
            let _ = handle.update(cx, |view, _, cx| view.popout_pin_changed(pinned, cx));
            {
                let mut settings = settings.borrow_mut();
                settings.update(room, |prefs| {
                    prefs.open = true;
                    prefs.pinned = pinned;
                });
                settings.sample(room, hwnd);
                settings.save();
            }
            if focus {
                win::show_and_focus(hwnd);
            } else {
                win::show_without_focus(hwnd);
            }
        })
        .detach();
    }

    /// Syncs open pop-out list windows when Windows Transparency effects change.
    pub fn set_material(&self, material: Material, cx: &mut App) {
        for handle in self.windows.values() {
            let _ = handle.update(cx, |view, window, cx| {
                window.set_background_appearance(material.window_background());
                view.set_material(material, cx);
            });
        }
    }

    /// Quit and sign-out don't count as explicitly closing a room. Save the last placement; if
    /// sign-out fails and resumes this pairing, its windows can be restored as before.
    pub fn close_all(&mut self, cx: &mut App) {
        for (room, handle) in self.windows.drain() {
            let _ = handle.update(cx, |_, window, _| {
                let hwnd = win::hwnd_of(window);
                if win::is_shown(hwnd) {
                    self.settings.borrow_mut().sample(room, hwnd);
                }
                window.remove_window();
            });
        }
        self.settings.borrow_mut().save();
    }
}

fn window_options(cx: &App) -> WindowOptions {
    let logical = size(px(SIZE.0), px(SIZE.1));
    WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::centered(
            None, logical, cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("To-do list".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        window_background: Material::system().window_background(),
        show: false,
        focus: false,
        app_id: Some("to-do-or-not-to-do".into()),
        ..WindowOptions::default()
    }
}
