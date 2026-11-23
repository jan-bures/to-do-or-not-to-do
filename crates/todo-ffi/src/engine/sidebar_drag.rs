//! Sidebar gesture bindings. The shared client owns hit interpretation and placement.

use std::sync::Arc;

use todo_client::sidebar_drag::{self, Drag, Row, Target};

use super::{TodoEngine, Undone, organize::OrganizeChange};
use crate::{FfiError, view::RoomKey};

#[derive(Debug, uniffi::Object)]
pub struct SidebarDrag(Drag);

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SidebarDrop {
    Beside { row: RoomKey, after: bool },
    Into { collection: String },
}

fn row(room: RoomKey) -> Option<Row> {
    match room {
        RoomKey::List { id } => Some(Row::List(id.parse().ok()?)),
        RoomKey::Collection { id } => Some(Row::Collection(id.parse().ok()?)),
        _ => None,
    }
}

fn room(row: Row) -> RoomKey {
    match row {
        Row::List(id) => RoomKey::List { id: id.to_string() },
        Row::Collection(id) => RoomKey::Collection { id: id.to_string() },
    }
}

impl SidebarDrop {
    fn target(self) -> Option<Target> {
        Some(match self {
            Self::Beside { row: key, after } => {
                let anchor = row(key)?;
                if after {
                    Target::After(anchor)
                } else {
                    Target::Before(anchor)
                }
            }
            Self::Into { collection } => Target::Into(collection.parse().ok()?),
        })
    }
}

#[uniffi::export]
impl TodoEngine {
    pub fn start_sidebar_drag(&self, source: RoomKey) -> Option<Arc<SidebarDrag>> {
        Drag::start(&self.client.view(), row(source)?).map(|drag| Arc::new(SidebarDrag(drag)))
    }

    pub fn sidebar_drop_target(
        &self,
        drag: Arc<SidebarDrag>,
        hit: RoomKey,
        after: bool,
    ) -> Option<SidebarDrop> {
        Some(match drag.0.hit(&self.client.view(), row(hit)?, after)? {
            Target::Before(anchor) => SidebarDrop::Beside {
                row: room(anchor),
                after: false,
            },
            Target::After(anchor) => SidebarDrop::Beside {
                row: room(anchor),
                after: true,
            },
            Target::Into(id) => SidebarDrop::Into {
                collection: id.to_string(),
            },
        })
    }

    pub fn drop_sidebar_row(
        &self,
        drag: Arc<SidebarDrag>,
        target: SidebarDrop,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let Some(target) = target.target() else {
            return Ok(None);
        };
        let Some(command) = drag.0.command(&self.client.view(), target) else {
            return Ok(None);
        };
        let label = sidebar_drag::label(&command).to_owned();
        let expected = command.clone();
        let tracked = self.client.submit_tracked_if(command, |state| {
            drag.0.command(state, target) == Some(expected)
        })?;
        Ok(tracked.map(|tracked| {
            Arc::new(OrganizeChange::new(
                tracked,
                Undone::Change("Undo moving a room", "room"),
                label,
            ))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::LocalNow;
    use todo_client::intent;
    use todo_core::time::Timestamp;

    #[test]
    fn sidebar_drop_is_one_undoable_edit_and_ignores_smart_and_own_slots() {
        let e = TodoEngine::fixture(
            "world".into(),
            LocalNow {
                epoch_ms: Timestamp::UNIX_EPOCH.as_millisecond(),
                utc_offset_secs: 0,
                zone_id: "UTC".into(),
                local_day: None,
            },
        )
        .unwrap();
        let before = e.state();
        let groups = intent::list_groups(&before);
        let source = groups[0].1[0];
        let collection = groups[1].0.expect("second collection");
        let source = room(Row::List(source));
        let destination = room(Row::Collection(collection));
        let drag = e.start_sidebar_drag(source.clone()).unwrap();
        assert!(e.start_sidebar_drag(RoomKey::Trash).is_none());
        assert!(
            e.sidebar_drop_target(drag.clone(), RoomKey::Trash, false)
                .is_none()
        );
        assert!(e.sidebar_drop_target(drag.clone(), source, true).is_none());
        let target = e
            .sidebar_drop_target(drag.clone(), destination.clone(), false)
            .unwrap();
        assert_eq!(
            target,
            SidebarDrop::Into {
                collection: collection.to_string()
            }
        );
        let change = e.drop_sidebar_row(drag, target).unwrap().unwrap();
        assert_eq!(e.client.pending().len(), 1);
        assert_ne!(intent::list_groups(&e.state()), groups);
        e.undo_organize(change).unwrap();
        assert_eq!(intent::list_groups(&e.state()), groups);

        let order = intent::organize::collections(&before);
        let drag = e
            .start_sidebar_drag(room(Row::Collection(order[0])))
            .unwrap();
        let target = e
            .sidebar_drop_target(drag.clone(), destination, true)
            .unwrap();
        let change = e.drop_sidebar_row(drag, target).unwrap().unwrap();
        assert_eq!(intent::organize::collections(&e.state())[0], order[1]);
        e.undo_organize(change).unwrap();
        assert_eq!(intent::organize::collections(&e.state()), order);
    }
}
