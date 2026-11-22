//! Metadata comes from the replica. Content uses the paired transport directly, off the UI thread.
#[cfg(test)]
mod tests;

use super::{TodoEngine, invalid, organize::OrganizeChange};
use crate::{FfiError, runtime::runtime, transport::AppTransport};
use std::{path::Path, sync::Arc};
use todo_client::{
    failure, intent,
    native::attachments::{FileError, FileTransport, MAX_BYTES},
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

#[uniffi::export]
impl TodoEngine {
    /// The destination is an already-created private temporary file owned by the app.
    pub fn download_attachment(&self, id: String, path: String) -> Result<(), FfiError> {
        const OFFLINE: &str = "Attachments download when connected to a server.";
        let state = self.state();
        let id = id.parse().map_err(|_| invalid("No such attachment."))?;
        let file = state
            .attachments
            .get(&id)
            .filter(|a| a.deleted.is_none())
            .ok_or_else(|| invalid("No such attachment."))?;
        let AppTransport::Http(http) = self.client.transport() else {
            return Err(invalid(OFFLINE));
        };
        runtime()?
            .block_on(http.download_to(id, file.size_bytes, Path::new(&path)))
            .map_err(|e| {
                invalid(match e {
                    FileError::Transport(_) => OFFLINE,
                    FileError::Refused { status: 404, .. } => {
                        "This attachment is no longer available."
                    }
                    _ => "Couldn’t download the attachment.",
                })
            })
    }

    pub fn upload_attachment(
        &self,
        task: String,
        name: String,
        media: String,
        path: String,
    ) -> Result<(), FfiError> {
        const OFFLINE: &str = "Attachments upload when connected to a server.";
        let (task, _) = self.live_task(&task)?;
        let AppTransport::Http(http) = self.client.transport() else {
            return Err(invalid(OFFLINE));
        };
        if !self.client.with_confirmed(|s| s.is_live(task.into())) {
            return Err(invalid("Wait for this task to sync, then add the file."));
        }
        let id = AttachmentId::from_uuid(self.client.new_uuid());
        runtime()?
            .block_on(http.upload_from(task, id, &name, &media, Path::new(&path)))
            .map_err(|e| {
                invalid(match e {
                    FileError::Transport(_) => OFFLINE.into(),
                    FileError::Refused { status, max_bytes } => {
                        failure::attachment_upload(Some(status), max_bytes)
                    }
                    _ => failure::attachment_upload(None, None),
                })
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
