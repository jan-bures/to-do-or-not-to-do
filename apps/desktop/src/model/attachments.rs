//! Task-scoped transfer state and the order of controls in the attachments section.

use std::collections::BTreeMap;

use todo_core::ids::{AttachmentId, TaskId};

use super::{AttachmentRow, files};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Control {
    Add,
    Save(AttachmentId),
    Open(AttachmentId),
    Remove(AttachmentId),
}

pub fn file_controls(rows: &[AttachmentRow]) -> impl Iterator<Item = Control> {
    rows.iter().flat_map(|row| {
        [
            Some(Control::Save(row.id)),
            files::can_open(&row.name).then_some(Control::Open(row.id)),
            Some(Control::Remove(row.id)),
        ]
        .into_iter()
        .flatten()
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Upload {
    Picking,
    Transferring,
    Notice { text: String, error: bool },
}

#[derive(Debug, Default)]
pub struct Uploads(BTreeMap<TaskId, Upload>);

impl Uploads {
    pub fn get(&self, task: TaskId) -> Option<&Upload> {
        self.0.get(&task)
    }

    pub fn busy(&self, task: TaskId) -> bool {
        matches!(self.get(task), Some(Upload::Picking | Upload::Transferring))
    }

    pub fn start(&mut self, task: TaskId, picking: bool) -> bool {
        if self.busy(task) {
            return false;
        }
        self.0.insert(
            task,
            if picking {
                Upload::Picking
            } else {
                Upload::Transferring
            },
        );
        true
    }

    pub fn finish(&mut self, task: TaskId, open: Option<TaskId>, notice: Option<(String, bool)>) {
        self.0.remove(&task);
        if open == Some(task)
            && let Some((text, error)) = notice
        {
            self.0.insert(task, Upload::Notice { text, error });
        }
    }

    pub fn select(&mut self, open: Option<TaskId>) {
        // Switching details never cancels a transfer, but an old notice isn't shown on a new visit.
        self.0
            .retain(|task, state| Some(*task) == open || !matches!(state, Upload::Notice { .. }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picker_and_transfer_reserve_only_their_original_task() {
        let fixture = crate::fixtures::my_day();
        let a = fixture.ui.task.expect("selected task");
        let b = fixture
            .state
            .tasks
            .keys()
            .copied()
            .find(|&id| id != a)
            .expect("another task");
        let mut uploads = Uploads::default();
        assert!(uploads.start(a, true));
        assert_eq!(uploads.get(a), Some(&Upload::Picking));
        assert!(
            !uploads.start(a, false),
            "a drop cannot bypass a picker reservation"
        );
        uploads.select(Some(b));
        assert!(uploads.busy(a));
        assert!(uploads.start(b, false));
        uploads.finish(a, Some(b), Some(("An error on A".into(), true)));
        assert_eq!(uploads.get(a), None);
        assert_eq!(uploads.get(b), Some(&Upload::Transferring));
        uploads.finish(b, Some(b), Some(("An error on B".into(), true)));
        assert!(matches!(
            uploads.get(b),
            Some(Upload::Notice { error: true, .. })
        ));
        assert!(!uploads.busy(b));
        uploads.select(Some(a));
        assert_eq!(uploads.get(b), None);
    }

    #[test]
    fn only_supported_files_offer_open_but_every_file_can_be_saved_or_removed() {
        let mut row = crate::fixtures::my_day()
            .shell()
            .detail
            .expect("detail")
            .attachments
            .remove(0);
        let id = row.id;
        row.name = "sample.pdf".into();
        assert_eq!(
            file_controls(std::slice::from_ref(&row)).collect::<Vec<_>>(),
            [Control::Save(id), Control::Open(id), Control::Remove(id)]
        );
        row.name = "sample.pdf.exe".into();
        assert_eq!(
            file_controls(&[row]).collect::<Vec<_>>(),
            [Control::Save(id), Control::Remove(id)]
        );
    }
}
