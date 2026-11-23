//! Trash rows and restore gestures over the same replica as the task rooms.

use super::*;
use crate::engine::organize::OrganizeChange;
use todo_client::trash;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TrashKind {
    Task,
    List,
    Collection,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TrashRow {
    pub kind: TrashKind,
    pub id: String,
    /// An exact timestamp identifying the deletion the owner saw.
    pub deleted_at: String,
    pub title: String,
    pub metadata: String,
    pub blocked: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TrashView {
    pub description: String,
    pub rows: Vec<TrashRow>,
}

impl TrashView {
    pub(crate) fn from_state(state: &State, now: Timestamp) -> Self {
        Self {
            description: trash::DESCRIPTION.into(),
            rows: trash::rows(state, now)
                .into_iter()
                .filter_map(|row| {
                    let (kind, id) = match row.id {
                        EntityId::Task(id) => (TrashKind::Task, id.to_string()),
                        EntityId::List(id) => (TrashKind::List, id.to_string()),
                        EntityId::Collection(id) => (TrashKind::Collection, id.to_string()),
                        _ => return None,
                    };
                    Some(TrashRow {
                        kind,
                        id,
                        deleted_at: row.deleted_at.to_string(),
                        title: row.title,
                        metadata: row.metadata,
                        blocked: row.blocked.map(str::to_owned),
                    })
                })
                .collect(),
        }
    }
}

#[uniffi::export]
pub fn trash_title() -> String {
    trash::TITLE.into()
}

#[uniffi::export]
impl TodoEngine {
    /// Restore exactly the deletion the row showed. A deleted parent must be restored first.
    pub fn restore_trash(&self, row: TrashRow) -> Result<Arc<OrganizeChange>, FfiError> {
        let (id, noun): (EntityId, _) = match row.kind {
            TrashKind::Task => (task_id(&row.id)?.into(), "task"),
            TrashKind::List => (list_id(&row.id)?.into(), "list"),
            TrashKind::Collection => (collection_id(&row.id)?.into(), "collection"),
        };
        let deleted_at = row
            .deleted_at
            .parse()
            .map_err(|_| invalid(trash::CHANGED))?;
        let tracked = self
            .client
            .submit_tracked_if(Command::Restore { id }, |state| {
                trash::restore(state, id, deleted_at).is_some()
            })?
            .ok_or_else(|| invalid(trash::CHANGED))?;
        Ok(Arc::new(OrganizeChange::new(
            tracked,
            Undone::Change("Undo a restore", noun),
            trash::restored(&row.title),
        )))
    }
}
