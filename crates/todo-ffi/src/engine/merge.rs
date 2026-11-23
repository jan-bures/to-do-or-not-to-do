use std::sync::Arc;

use super::{TodoEngine, Undone, organize::OrganizeChange, task_id};
use crate::FfiError;
use todo_client::intent::merge;

#[derive(Debug, uniffi::Record)]
pub struct MergeCandidate {
    pub id: String,
    pub title: String,
    pub location: String,
}

#[derive(Debug, uniffi::Enum)]
pub enum MergeResult {
    Merged { change: Arc<OrganizeChange> },
    Changed { question: Option<String> },
}

#[uniffi::export]
pub fn merge_empty_text(filtered: bool) -> String {
    if filtered {
        merge::NO_MATCHES
    } else {
        merge::EMPTY
    }
    .into()
}

#[uniffi::export]
pub fn merge_changed_text() -> String {
    merge::CHANGED.into()
}

#[uniffi::export]
impl TodoEngine {
    pub fn merge_candidates(&self, source: String, filter: String) -> Vec<MergeCandidate> {
        task_id(&source)
            .ok()
            .and_then(|id| merge::candidates(&self.client.view(), id, &filter))
            .unwrap_or_default()
            .into_iter()
            .map(|c| MergeCandidate {
                id: c.id.to_string(),
                title: c.title,
                location: c.location,
            })
            .collect()
    }

    pub fn merge_question(&self, source: String, target: String) -> Option<String> {
        let preview = merge::preview(
            &self.client.view(),
            task_id(&source).ok()?,
            task_id(&target).ok()?,
        )?;
        Some(preview.question)
    }

    /// Recheck the confirmed choice under the sync lock, behind any writes already queued.
    pub fn merge_tasks(
        &self,
        source: String,
        target: String,
        asked: String,
    ) -> Result<MergeResult, FfiError> {
        let (source, target) = (task_id(&source)?, task_id(&target)?);
        let state = self.client.view();
        if let Some(preview) = merge::preview(&state, source, target)
            && let Some(cmd) = merge::command(&state, source, target, &asked)
            && let Some(tracked) = self
                .client
                .submit_tracked_if(cmd, |s| merge::command(s, source, target, &asked).is_some())?
        {
            return Ok(MergeResult::Merged {
                change: Arc::new(OrganizeChange::new(
                    tracked,
                    Undone::Change("Undo merging tasks", "task"),
                    preview.toast,
                )),
            });
        }
        Ok(MergeResult::Changed {
            question: merge::preview(&self.client.view(), source, target).map(|p| p.question),
        })
    }
}
