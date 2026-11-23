//! Android bindings for workflow settings, commands and projections. No UI state lives here.

use std::sync::Arc;

use todo_client::{
    board,
    intent::{self, workflow},
    timeline,
};
use todo_core::{command, command::Placement, model, query::QueryStatus, time::Date};

use super::{TodoEngine, Undone, intent_room, invalid, list_id, organize::OrganizeChange, task_id};
use crate::{
    FfiError,
    labels::{LocalNow, Now},
    quick_add::WorkflowQuickAddDraft,
    view::{self, RoomKey},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ListView {
    List,
    Board,
    Timeline,
}

impl From<ListView> for model::ListView {
    fn from(view: ListView) -> Self {
        match view {
            ListView::List => Self::List,
            ListView::Board => Self::Board,
            ListView::Timeline => Self::Timeline,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ListFeatures {
    pub workflow: bool,
    pub timeline: bool,
    pub default_view: ListView,
}

impl From<model::ListFeatures> for ListFeatures {
    fn from(f: model::ListFeatures) -> Self {
        Self {
            workflow: f.workflow,
            timeline: f.timeline,
            default_view: match f.default_view {
                model::ListView::List => ListView::List,
                model::ListView::Board => ListView::Board,
                model::ListView::Timeline => ListView::Timeline,
            },
        }
    }
}

impl From<ListFeatures> for model::ListFeatures {
    fn from(f: ListFeatures) -> Self {
        Self {
            workflow: f.workflow,
            timeline: f.timeline,
            default_view: f.default_view.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct ListFeaturesPatch {
    pub workflow: Option<bool>,
    pub timeline: Option<bool>,
    pub default_view: Option<ListView>,
}

impl From<ListFeaturesPatch> for command::ListFeaturesPatch {
    fn from(patch: ListFeaturesPatch) -> Self {
        Self {
            workflow: patch.workflow,
            timeline: patch.timeline,
            default_view: patch.default_view.map(Into::into),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OpenStage {
    Backlog,
    Todo,
    InProgress,
}

impl From<OpenStage> for model::WorkflowStatus {
    fn from(s: OpenStage) -> Self {
        match s {
            OpenStage::Backlog => Self::Backlog,
            OpenStage::Todo => Self::Todo,
            OpenStage::InProgress => Self::InProgress,
        }
    }
}

impl From<model::WorkflowStatus> for OpenStage {
    fn from(s: model::WorkflowStatus) -> Self {
        match s {
            model::WorkflowStatus::Backlog => Self::Backlog,
            model::WorkflowStatus::Todo => Self::Todo,
            model::WorkflowStatus::InProgress => Self::InProgress,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WorkflowStage {
    Backlog,
    Todo,
    InProgress,
    Done,
}

impl From<QueryStatus> for WorkflowStage {
    fn from(s: QueryStatus) -> Self {
        match s {
            QueryStatus::Backlog => Self::Backlog,
            QueryStatus::Todo => Self::Todo,
            QueryStatus::InProgress => Self::InProgress,
            QueryStatus::Done => Self::Done,
        }
    }
}

impl From<WorkflowStage> for QueryStatus {
    fn from(s: WorkflowStage) -> Self {
        match s {
            WorkflowStage::Backlog => Self::Backlog,
            WorkflowStage::Todo => Self::Todo,
            WorkflowStage::InProgress => Self::InProgress,
            WorkflowStage::Done => Self::Done,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskWorkflow {
    pub features: ListFeatures,
    pub stage: WorkflowStage,
    pub open_stage: OpenStage,
    pub start_date: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BoardColumn {
    pub stage: WorkflowStage,
    pub task_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BoardView {
    pub columns: Vec<BoardColumn>,
    pub total_done: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BoardSlot {
    First,
    Last,
    Before { task_id: String },
    After { task_id: String },
}

impl BoardSlot {
    fn placement(self) -> Result<Placement, FfiError> {
        Ok(match self {
            Self::First => Placement::First,
            Self::Last => Placement::Last,
            Self::Before { task_id: id } => Placement::Before(task_id(&id)?.into()),
            Self::After { task_id: id } => Placement::After(task_id(&id)?.into()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TimelineBar {
    Range,
    Milestone,
    OpenEnded,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TimelineLane {
    pub task_id: String,
    pub list_id: String,
    pub stage: WorkflowStage,
    pub first_date: String,
    pub last_date: String,
    pub bar: TimelineBar,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TimelineView {
    pub lanes: Vec<TimelineLane>,
    pub unscheduled: Vec<String>,
}

#[uniffi::export]
impl TodoEngine {
    pub fn list_features(&self, id: String) -> Option<ListFeatures> {
        let (id, state) = self.live_list(&id).ok()?;
        Some(state.lists[&id].features.into())
    }

    /// Change only the settings supplied by the caller.
    pub fn set_list_features(
        &self,
        id: String,
        features: ListFeaturesPatch,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let (id, state) = self.live_list(&id)?;
        self.track_change(
            workflow::set_list_features(&state, id, features.into()),
            "Undo changing list settings",
            "list",
            "List settings changed".into(),
        )
    }

    pub fn task_workflow(&self, id: String) -> Option<TaskWorkflow> {
        let (id, state) = self.live_task(&id).ok()?;
        let task = &state.tasks[&id];
        Some(TaskWorkflow {
            features: state.lists.get(&task.list)?.features.into(),
            stage: QueryStatus::of(task).into(),
            open_stage: task.status.into(),
            start_date: task.start_date.map(|d| d.to_string()),
        })
    }

    /// Edits the stored open stage. Use completion APIs for Done, or the board drop to reopen and move.
    pub fn set_task_status(
        &self,
        id: String,
        status: OpenStage,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let (id, state) = self.live_task(&id)?;
        self.track_change(
            workflow::set_status(&state, id, status.into()),
            "Undo changing status",
            "task",
            "Status changed".into(),
        )
    }

    pub fn set_start_date(
        &self,
        id: String,
        date: Option<String>,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let date: Option<Date> = date
            .map(|d| d.trim().parse())
            .transpose()
            .map_err(|_| invalid("That isn't a date."))?;
        let (id, state) = self.live_task(&id)?;
        self.track_change(
            workflow::set_start(&state, id, date),
            "Undo changing start date",
            "task",
            "Start date changed".into(),
        )
    }

    pub fn board(&self, list: String, now: LocalNow) -> Option<BoardView> {
        let board = board::project(
            &self.client.view(),
            list_id(&list).ok()?,
            &Now::new(now).eval(),
        )?;
        Some(BoardView {
            columns: board
                .columns
                .into_iter()
                .map(|column| BoardColumn {
                    stage: column.stage.into(),
                    task_ids: column.cards.into_iter().map(|id| id.to_string()).collect(),
                })
                .collect(),
            total_done: u32::try_from(board.total_done).unwrap_or(u32::MAX),
        })
    }

    /// Complete, reorder or move a card across columns as a single tracked change.
    pub fn drop_board_card(
        &self,
        list: String,
        card: String,
        stage: WorkflowStage,
        slot: BoardSlot,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let (list, card, stage, slot) = (
            list_id(&list)?,
            task_id(&card)?,
            stage.into(),
            slot.placement()?,
        );
        let Some(cmd) = board::drop_command(&self.client.view(), list, card, stage, slot) else {
            return Ok(None);
        };
        let tracked = self.client.submit_tracked_if(cmd.clone(), |state| {
            board::drop_command(state, list, card, stage, slot).as_ref() == Some(&cmd)
        })?;
        Ok(tracked.map(|t| {
            Arc::new(OrganizeChange::new(
                t,
                Undone::Change("Undo moving a card", "task"),
                "Card moved".into(),
            ))
        }))
    }

    pub fn timeline(&self, room: RoomKey, now: LocalNow) -> Option<TimelineView> {
        let timeline = timeline::room(
            &self.client.view(),
            view::shared_key(&room)?,
            &Now::new(now).eval(),
        )?;
        Some(TimelineView {
            lanes: timeline
                .lanes
                .into_iter()
                .map(|lane| TimelineLane {
                    task_id: lane.task.to_string(),
                    list_id: lane.list.to_string(),
                    stage: lane.stage.into(),
                    first_date: lane.span.first.to_string(),
                    last_date: lane.span.last.to_string(),
                    bar: match lane.span.kind {
                        timeline::SpanKind::Range => TimelineBar::Range,
                        timeline::SpanKind::Milestone => TimelineBar::Milestone,
                        timeline::SpanKind::OpenEnded => TimelineBar::OpenEnded,
                    },
                })
                .collect(),
            unscheduled: timeline
                .unscheduled
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
        })
    }

    pub fn workflow_quick_add_features(&self, room: RoomKey) -> ListFeatures {
        intent::quick_add_features(&self.client.view(), intent_room(room)).into()
    }

    pub fn add_workflow_task(
        &self,
        room: RoomKey,
        draft: WorkflowQuickAddDraft,
        now: LocalNow,
    ) -> Result<String, FfiError> {
        let (id, commands) = intent::submit_workflow_draft(
            &self.client.view(),
            intent_room(room),
            &draft.into(),
            &Now::new(now).eval(),
            || self.client.new_uuid(),
        );
        let id = id.ok_or_else(|| invalid("A task needs a title."))?;
        for command in commands {
            self.client.submit(command)?;
        }
        Ok(id.to_string())
    }
}
