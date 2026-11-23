//! Task drags are planned by todo-client against the current replica, not by the gesture layer.

use std::sync::Arc;

use todo_client::task_drag::{Destination, Drag, Slot};
use todo_core::{Command, State};

use super::{TodoEngine, Undone, invalid, list_id, organize::OrganizeChange, task_id};
use crate::{FfiError, view::RoomKey};

#[derive(Debug, uniffi::Object)]
pub struct TaskDrag(Drag);

#[uniffi::export]
impl TaskDrag {
    pub fn ids(&self) -> Vec<String> {
        self.0.ids().map(|id| id.to_string()).collect()
    }
}

#[derive(Clone, Debug, uniffi::Enum)]
pub enum TaskDrop {
    MyDay,
    List { id: String },
    Slot { row: String, after: bool },
}

#[uniffi::export]
impl TodoEngine {
    pub fn start_task_drag(&self, source: String, selected: Vec<String>) -> Option<Arc<TaskDrag>> {
        let selected = selected
            .iter()
            .map(|id| task_id(id))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Drag::start(&self.client.view(), task_id(&source).ok()?, &selected)
            .map(|drag| Arc::new(TaskDrag(drag)))
    }

    pub fn task_drop_valid(
        &self,
        drag: Arc<TaskDrag>,
        target: TaskDrop,
        room: RoomKey,
        open: Vec<String>,
        today: String,
    ) -> bool {
        let state = self.client.view();
        let Some(plan) = DropPlan::new(target, room, &open, &today) else {
            return false;
        };
        match plan {
            DropPlan::Destination(dest, day) => drag.0.eligible(&state, dest, day),
            DropPlan::Slot(list, open, slot) => {
                drag.0.placement(&state, list, &open, slot).is_some()
            }
        }
    }

    pub fn drop_tasks(
        &self,
        drag: Arc<TaskDrag>,
        target: TaskDrop,
        room: RoomKey,
        open: Vec<String>,
        today: String,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let Some(plan) = DropPlan::new(target, room, &open, &today) else {
            return Ok(None);
        };
        let Some((cmd, label)) = plan.command(&self.client.view(), &drag.0)? else {
            return Ok(None);
        };
        let tracked = self.client.submit_tracked_if(cmd.clone(), |state| {
            plan.command(state, &drag.0)
                .ok()
                .flatten()
                .is_some_and(|(live, _)| live == cmd)
        })?;
        Ok(tracked.map(|target| {
            Arc::new(OrganizeChange::new(
                target,
                Undone::Change("Undo organizing tasks", "task"),
                label,
            ))
        }))
    }
}

enum DropPlan {
    Destination(Destination, todo_core::time::Date),
    Slot(
        Option<todo_core::ids::ListId>,
        Vec<todo_core::ids::TaskId>,
        Slot,
    ),
}

impl DropPlan {
    fn new(target: TaskDrop, room: RoomKey, open: &[String], today: &str) -> Option<Self> {
        Some(match target {
            TaskDrop::MyDay => Self::Destination(Destination::MyDay, today.parse().ok()?),
            TaskDrop::List { id } => {
                Self::Destination(Destination::List(list_id(&id).ok()?), today.parse().ok()?)
            }
            TaskDrop::Slot { row, after } => {
                let RoomKey::List { id } = room else {
                    return None;
                };
                Self::Slot(
                    Some(list_id(&id).ok()?),
                    open.iter()
                        .map(|id| task_id(id))
                        .collect::<Result<_, _>>()
                        .ok()?,
                    Slot {
                        row: task_id(&row).ok()?,
                        after,
                    },
                )
            }
        })
    }

    fn command(&self, state: &State, drag: &Drag) -> Result<Option<(Command, String)>, FfiError> {
        match self {
            Self::Destination(dest, day) => drag.drop_on(state, *dest, *day),
            Self::Slot(list, open, slot) => drag.drop_in(state, *list, open, *slot),
        }
        .map_err(|e| invalid(e.to_string()))
    }
}
