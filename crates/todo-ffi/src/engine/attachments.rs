//! Metadata comes from the replica. Content uses the paired transport directly, off the UI thread.
#[cfg(test)]
mod tests;

use super::{TodoEngine, invalid, organize::OrganizeChange};
use crate::{FfiError, runtime::runtime, transport::AppTransport};
use std::{path::Path, sync::Arc};
use todo_client::{
    failure, intent,
    native::attachments::{FileTransport, MAX_BYTES},
};
use todo_core::{ids::AttachmentId, model::AttachmentRef};

#[uniffi::export]
pub fn attachment_limit_bytes() -> u64 {
    MAX_BYTES
}

#[uniffi::export]
pub fn attachment_too_large() -> String {
    failure::attachment_upload(Some(413), Some(MAX_BYTES)).into_owned()
}

/// Display and intent metadata only: neither a server address nor a storage key crosses the FFI.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttachmentRow {
    pub id: String,
    pub name: String,
    pub size: String,
    pub icon: String,
    /// Deliberately based on the extension, not on a supplied MIME type. Unknown files are save-only.
    pub open_type: Option<String>,
}

impl From<&AttachmentRef> for AttachmentRow {
    fn from(file: &AttachmentRef) -> Self {
        let base = file
            .file_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default();
        let name: String = base.chars().filter(|c| !c.is_control() && !matches!(*c,
            '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')).collect();
        let name = name.trim().trim_end_matches(['.', ' ']);
        let name = if name.is_empty() { "attachment" } else { name };
        let ext = name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map(|(_, ext)| ext.to_ascii_lowercase());
        let media = match ext.as_deref() {
            Some("pdf") => Some("application/pdf"),
            Some("txt" | "md") => Some("text/plain"),
            Some("csv") => Some("text/csv"),
            Some("png") => Some("image/png"),
            Some("jpg" | "jpeg") => Some("image/jpeg"),
            Some("webp") => Some("image/webp"),
            Some("gif") => Some("image/gif"),
            Some("eml") => Some("message/rfc822"),
            Some("msg") => Some("application/vnd.ms-outlook"),
            Some("docx") => {
                Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
            }
            Some("xlsx") => {
                Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
            }
            Some("pptx") => {
                Some("application/vnd.openxmlformats-officedocument.presentationml.presentation")
            }
            _ => None,
        };
        let icon = match media {
            Some("message/rfc822" | "application/vnd.ms-outlook") => "mail",
            Some(m) if m.starts_with("image/") => "image",
            Some(_) => "text",
            None => "paperclip",
        };
        let bytes = file.size_bytes;
        #[allow(clippy::cast_precision_loss)]
        let kb = bytes as f64 / 1024.0;
        let size = if bytes < 1024 {
            format!("{bytes} B")
        } else if kb < 1024.0 {
            format!("{kb:.0} KB")
        } else {
            format!("{:.1} MB", kb / 1024.0)
        };
        Self {
            id: file.id.to_string(),
            name: name.into(),
            icon: icon.into(),
            size,
            open_type: media.map(str::to_owned),
        }
    }
}

impl TodoEngine {
    fn transfer<T>(
        &self,
        epoch: u64,
        work: impl Future<Output = Result<T, FfiError>>,
    ) -> Result<T, FfiError> {
        let mut stop = self.transfer_epoch.subscribe();
        let cancelled = || invalid("Transfer cancelled.");
        if *stop.borrow() != epoch {
            return Err(cancelled());
        }
        runtime()?.block_on(async {
            tokio::select! { biased;
                _ = stop.changed() => Err(cancelled()),
                result = work => result,
            }
        })
    }
}

#[uniffi::export]
impl TodoEngine {
    /// Invalidate staging work and drop active HTTP futures even if a rapid resume keeps sync running.
    pub fn cancel_attachment_transfers(&self) {
        self.transfer_epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    pub fn attachment_epoch(&self) -> u64 {
        *self.transfer_epoch.borrow()
    }

    pub fn attachment_task_confirmed(&self, task: String) -> bool {
        super::task_id(&task)
            .is_ok_and(|task| self.client.with_confirmed(|s| s.is_live(task.into())))
    }

    /// The destination is an already-created private temporary file owned by the app.
    pub fn download_attachment(
        &self,
        id: String,
        path: String,
        epoch: u64,
    ) -> Result<(), FfiError> {
        let state = self.state();
        let id = id.parse().map_err(|_| invalid("No such attachment."))?;
        let file = state
            .attachments
            .get(&id)
            .filter(|a| a.deleted.is_none())
            .ok_or_else(|| invalid("No such attachment."))?;
        let AppTransport::Http(http) = self.client.transport() else {
            return Err(invalid(failure::ATTACHMENT_OFFLINE));
        };
        self.transfer(epoch, async {
            http.download_to(id, file.size_bytes, Path::new(&path))
                .await
                .map_err(|e| invalid(e.download_message()))
        })
    }

    pub fn upload_attachment(
        &self,
        task: String,
        name: String,
        media: String,
        path: String,
        epoch: u64,
    ) -> Result<String, FfiError> {
        let (task, _) = self.live_task(&task)?;
        let AppTransport::Http(http) = self.client.transport() else {
            return Err(invalid(failure::ATTACHMENT_OFFLINE));
        };
        if !self.client.with_confirmed(|s| s.is_live(task.into())) {
            return Err(invalid("Wait for this task to sync, then add the file."));
        }
        let id = AttachmentId::from_uuid(self.client.new_uuid());
        self.transfer(epoch, async {
            http.upload_from(task, id, &name, &media, Path::new(&path))
                .await
                .map_err(|e| invalid(e.upload_message()))
        })?;
        self.client.nudge();
        Ok(id.to_string())
    }

    /// Acceptance is not visibility. Wait for SSE/catch-up, including an immediate remote deletion.
    pub fn await_attachment(&self, id: String, epoch: u64) -> Result<bool, FfiError> {
        let id: AttachmentId = id.parse().map_err(|_| invalid("No such attachment."))?;
        self.transfer(epoch, async {
            let mut changes = self.client.changes();
            let observed = async {
                while !self
                    .client
                    .with_confirmed(|s| s.attachments.contains_key(&id))
                {
                    changes.next().await;
                }
            };
            Ok(
                tokio::time::timeout(std::time::Duration::from_secs(30), observed)
                    .await
                    .is_ok(),
            )
        })
    }

    pub fn remove_attachment(&self, id: String) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let id = id.parse().map_err(|_| invalid("No such attachment."))?;
        let s = self.state();
        let Some(file) = s.attachments.get(&id).filter(|a| a.deleted.is_none()) else {
            return Ok(None);
        };
        self.track_change(
            Some(intent::delete(id)),
            "Undo removing an attachment",
            "attachment",
            format!("Removed attachment “{}”", AttachmentRow::from(file).name),
        )
    }
}
