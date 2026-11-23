//! The browser owns the physical stack; the shared model identifies and validates this document's
//! recent entries. A pop never pushes a route, including when it skips a stale entry.

use leptos::prelude::*;
use todo_client::navigation::{self, Direction, History};
use todo_core::{State, model::ListView};

use super::{Ctx, RoomId, model, platform, workflow};

pub(super) type Location = navigation::Location<RoomId>;

pub(super) struct BrowserHistory {
    session: String,
    entries: History<Location>,
    resolving_initial: bool,
}

impl BrowserHistory {
    pub(super) fn start(state: &State, path: &str) -> (Self, Location, Option<ListView>) {
        let (location, requested) = parse(state, path);
        let history = Self {
            // Only a same-document marker, not a credential or a persisted ID.
            session: format!("todo:{}", js_sys::Math::random()),
            entries: History::new(location.clone()),
            resolving_initial: true,
        };
        platform::write_history(path, &history.marker(0), false);
        (history, location, requested)
    }

    fn marker(&self, position: usize) -> String {
        format!("{}:{position}", self.session)
    }

    fn position(&self, marker: &str) -> Option<usize> {
        let (session, position) = marker.rsplit_once(':')?;
        (session == self.session).then_some(())?;
        let position = position.parse().ok()?;
        self.entries.get(position)?;
        Some(position)
    }

    fn replace_url(&self, path: &str) {
        if platform::path() != path {
            platform::write_history(path, &self.marker(self.entries.position()), false);
        }
    }
}

fn parse(state: &State, path: &str) -> (Location, Option<ListView>) {
    let (base, requested) = match path.split_once("/view/") {
        Some((room, tail)) => {
            let (name, task) = tail.split_once('/').unwrap_or((tail, ""));
            let view = match name {
                "list" => Some(ListView::List),
                "board" => Some(ListView::Board),
                "timeline" => Some(ListView::Timeline),
                _ => None,
            };
            (format!("{room}/{task}"), view)
        }
        None => (path.to_owned(), None),
    };
    let (room, task) = RoomId::from_path(&base);
    let requested = requested.filter(|_| !room.is_history());
    let view =
        requested.unwrap_or_else(|| workflow::effective(workflow::features(state, &room), None));
    (Location { room, view, task }, requested)
}

fn path(state: &State, location: &Location) -> String {
    let mut path = location.room.path();
    let view = match location.view {
        ListView::List => {
            // Bare user-list links choose the synced default; an explicit List visit needs
            // a permanent address even if a different default is saved later.
            let features = workflow::features(state, &location.room);
            (matches!(location.room, RoomId::List(_)) && (features.workflow || features.timeline))
                .then_some("list")
        }
        ListView::Board => Some("board"),
        ListView::Timeline => Some("timeline"),
    };
    if let Some(view) = view {
        path.push_str("/view/");
        path.push_str(view);
    }
    if let Some(task) = location.task {
        path.push_str(&format!("/task/{task}"));
    }
    path
}

fn available(state: &State, location: &Location) -> bool {
    let room = model::shared_key(&location.room);
    navigation::room_exists(state, room)
        && navigation::view_exists(state, room, location.view)
        && location
            .task
            .is_none_or(|task| navigation::task_exists(state, task))
}

fn fallback(state: &State, mut location: Location) -> Location {
    let key = model::shared_key(&location.room);
    if !navigation::room_exists(state, key) {
        return Location {
            room: RoomId::default(),
            view: ListView::List,
            task: None,
        };
    }
    if !navigation::view_exists(state, key, location.view) {
        location.view = workflow::effective(workflow::features(state, &location.room), None);
    }
    location.task = location
        .task
        .filter(|&task| navigation::task_exists(state, task));
    location
}

impl Ctx {
    fn location(self) -> Location {
        Location {
            room: self.room.get_untracked(),
            view: self.room_view.get_untracked(),
            task: self.selected.get_untracked(),
        }
    }

    pub(super) fn record_location(self) {
        let location = self.location();
        let url = self.state.with_untracked(|s| path(s, &location));
        self.history.update_value(|history| {
            if history.entries.current() == &location {
                history.replace_url(&url);
            } else if platform::write_history(
                &url,
                &history.marker(history.entries.position() + 1),
                true,
            ) {
                history.entries.push(location);
                history.resolving_initial = false;
            }
        });
    }

    fn restore_location(self, location: Location) {
        if self.room.get_untracked() != location.room
            || self.room_view.get_untracked() != location.view
        {
            self.reset_room_controls();
        } else {
            self.multi.update(todo_client::selection::Selection::clear);
            self.close_bulk_due(false);
            self.room_actions.close_menu(false);
            let _ = self.task_actions.close();
            self.status_menu.close(false);
            self.drawer.set(false);
        }
        self.room.set(location.room);
        self.view_override.set(Some(location.view));
        self.set_detail(location.task);
    }

    pub(super) fn pop_location(self, marker: Option<&str>) {
        if self.sync.with_untracked(|s| s.signed_out) {
            return;
        }
        let position = self
            .history
            .with_value(|history| marker.and_then(|m| history.position(m)));
        let Some(position) = position else {
            // Reload, a browser entry older than the in-memory window, or a native deep link.
            // Its URL still works, but it doesn't belong to the current in-memory sequence.
            let (history, location, requested) = self
                .state
                .with_untracked(|s| BrowserHistory::start(s, &platform::path()));
            self.history.set_value(history);
            self.restore_location(location);
            self.view_override.set(requested);
            self.reconcile_location();
            return;
        };
        let state = self.state.get_untracked();
        let next = self
            .history
            .try_update_value(|history| {
                let current = history.entries.position();
                let direction = if position < current {
                    Direction::Back
                } else {
                    Direction::Forward
                };
                if position != current {
                    if let Some(target) = history
                        .entries
                        .find_from(position, direction, |l| available(&state, l))
                    {
                        if target != position && platform::traverse_history(position, target) {
                            return None;
                        }
                    } else if direction == Direction::Forward
                        && platform::traverse_history(position, current)
                    {
                        // There are no surviving forward destinations. Return to the current entry.
                        return None;
                    }
                    // At the oldest entry there may be no remaining app destination. Show a safe
                    // fallback here so the next Back can still leave the app.
                }
                let location = fallback(&state, history.entries.visit(position)?.clone());
                history.replace_url(&path(&state, &location));
                Some(location)
            })
            .flatten();
        if let Some(location) = next {
            self.restore_location(location);
        }
    }

    /// Resolve a cold deep link after catch-up; later removals update the address without creating
    /// an entry. Keep the original in memory so it is skipped on a later Back/Forward traversal.
    pub(super) fn reconcile_location(self) {
        if !self
            .sync
            .with_untracked(|s| !s.signed_out && (s.reminders_ready || s.offline_demo))
        {
            return;
        }
        let state = self.state.get_untracked();
        let resolved = self
            .history
            .try_update_value(|history| {
                if history.resolving_initial {
                    let mut location = history.entries.current().clone();
                    location.view = self.room_view.get_untracked();
                    let location = fallback(&state, location);
                    history.entries.resolve_initial(location.clone());
                    history.resolving_initial = false;
                    history.replace_url(&path(&state, &location));
                    Some(location)
                } else {
                    let location = history.entries.current();
                    if location.room != self.room.get_untracked() || available(&state, location) {
                        return None;
                    }
                    let repaired = fallback(&state, location.clone());
                    history.replace_url(&path(&state, &repaired));
                    let shown = self.location();
                    let repaired_shown = fallback(&state, shown.clone());
                    (shown != repaired_shown).then_some(repaired_shown)
                }
            })
            .flatten();
        if let Some(location) = resolved {
            self.restore_location(location);
        }
    }

    pub(super) fn replace_deleted_room(self, room: RoomId) {
        let state = self.state.get_untracked();
        let location = Location {
            view: workflow::effective(workflow::features(&state, &room), None),
            room,
            task: None,
        };
        self.history
            .with_value(|history| history.replace_url(&path(&state, &location)));
        self.restore_location(location);
    }
}
