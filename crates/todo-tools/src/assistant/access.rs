//! Assistant list and collection permissions, resolved against confirmed task state.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use todo_core::{
    State,
    ids::{CollectionId, ListId},
};

/// Where assistant inference happens. A private model reached through the paired server is Local.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    Local,
    Cloud,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListAccess {
    #[default]
    Hidden,
    #[serde(rename = "Private")]
    PrivateOnly,
    #[serde(rename = "All")]
    AllModels,
}

impl ListAccess {
    pub fn allows(self, data: DataClass) -> bool {
        match self {
            Self::Hidden => false,
            Self::PrivateOnly => data == DataClass::Local,
            Self::AllModels => true,
        }
    }
}

/// An explicit list override, including Hidden, follows the list across collection moves. Without
/// an override, the list inherits its current live collection's choice, or is hidden by default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRules {
    pub lists: BTreeMap<ListId, ListAccess>,
    #[serde(default)]
    pub collections: BTreeMap<CollectionId, ListAccess>,
}

impl AccessRules {
    pub fn list(&self, state: &State, id: ListId) -> ListAccess {
        let Some(list) = state.lists.get(&id).filter(|_| state.is_live(id.into())) else {
            return ListAccess::Hidden;
        };
        self.lists.get(&id).copied().unwrap_or_else(|| {
            list.collection
                .filter(|&collection| state.is_live(collection.into()))
                .and_then(|collection| self.collections.get(&collection).copied())
                .unwrap_or_default()
        })
    }

    /// The common access of the collection's live lists, or None for Mixed. An empty collection
    /// shows its stored default so it is possible to configure access before adding any lists.
    pub fn collection(&self, state: &State, id: CollectionId) -> Option<ListAccess> {
        let mut members = Self::members(state, id).map(|id| self.list(state, id));
        let common = members
            .next()
            .unwrap_or_else(|| self.collections.get(&id).copied().unwrap_or_default());
        members.all(|access| access == common).then_some(common)
    }

    /// Set the default for future members and replace all overrides for current live members.
    pub fn set_collection(&mut self, state: &State, id: CollectionId, access: ListAccess) {
        if state.is_live(id.into()) {
            self.collections.insert(id, access);
            for member in Self::members(state, id) {
                self.lists.remove(&member);
            }
        }
    }

    fn members(state: &State, id: CollectionId) -> impl Iterator<Item = ListId> {
        let live = state.is_live(id.into());
        state.lists.values().filter_map(move |list| {
            (live && list.collection == Some(id) && state.is_live(list.id.into()))
                .then_some(list.id)
        })
    }

    pub fn allowed(&self, state: &State, data: DataClass) -> BTreeSet<ListId> {
        state
            .lists
            .keys()
            .copied()
            .filter(|&id| self.list(state, id).allows(data))
            .collect()
    }
}

impl From<BTreeMap<ListId, ListAccess>> for AccessRules {
    fn from(lists: BTreeMap<ListId, ListAccess>) -> Self {
        Self {
            lists,
            ..Self::default()
        }
    }
}
