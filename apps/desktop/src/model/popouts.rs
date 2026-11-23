//! Device-local placement for list windows. A room is identified by its list ID, not its name.

use serde::{Deserialize, Serialize};
use todo_core::State;
use todo_core::ids::ListId;

use super::{RoomKey, Smart};

/// Preferred and minimum window sizes in logical pixels. Ordinary desktop row metrics still apply.
pub const SIZE: (f32, f32) = (480., 528.);
pub const MIN_SIZE: (f32, f32) = (400., 360.);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum Room {
    List(ListId),
    Smart(Smart),
}

impl Room {
    pub fn for_key(key: RoomKey) -> Option<Self> {
        match key {
            RoomKey::List(id) => Some(Self::List(id)),
            RoomKey::Smart(smart) => Some(Self::Smart(smart)),
            _ => None,
        }
    }

    pub fn key(self) -> RoomKey {
        match self {
            Self::List(id) => RoomKey::List(id),
            Self::Smart(smart) => RoomKey::Smart(smart),
        }
    }

    pub fn available(self, state: &State) -> bool {
        self.key().resolved(state) == self.key()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Physical screen coordinates (which may be negative on secondary displays).
    pub x: i32,
    pub y: i32,
    /// Logical outer size; convert at the destination monitor's current scale.
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preference {
    pub place: Option<Placement>,
    pub pinned: bool,
    pub open: bool,
}

impl Default for Preference {
    fn default() -> Self {
        Self {
            place: None,
            pinned: true,
            open: false,
        }
    }
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Preferences(Vec<(Room, Preference)>);

impl Preferences {
    pub fn get(&self, room: Room) -> Preference {
        self.0
            .iter()
            .find(|(key, _)| *key == room)
            .map(|(_, prefs)| *prefs)
            .unwrap_or_default()
    }

    pub fn set(&mut self, room: Room, prefs: Preference) -> bool {
        if self.get(room) == prefs {
            return false;
        }
        if let Some((_, current)) = self.0.iter_mut().find(|(key, _)| *key == room) {
            *current = prefs;
        } else {
            self.0.push((room, prefs));
        }
        true
    }

    pub fn open_rooms(&self) -> impl Iterator<Item = Room> + '_ {
        self.0
            .iter()
            .filter_map(|(room, prefs)| prefs.open.then_some(*room))
    }
}
