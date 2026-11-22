//! Direct, bounded file transfers. Neither the outbox nor the JVM heap holds the content.
use super::{HttpTransport, network};
use crate::platform::{Method, TransportError, attachment_upload_path};
use std::{future::Future, path::Path, time::Duration};
use todo_core::ids::{AttachmentId, TaskId};
use tokio::{fs::File, io::AsyncWriteExt};

/// Server hard ceiling; a server configured below it returns its actual limit with HTTP 413.
pub use todo_core::MAX_ATTACHMENT_BYTES as MAX_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("file transfer failed")]
    Transport(#[from] TransportError),
    #[error("could not read or write the file")]
    File(#[from] std::io::Error),
    #[error("the file transfer was refused (HTTP {status})")]
    Refused { status: u16, max_bytes: Option<u64> },
    #[error("the download size did not match the attachment")]
    WrongSize,
}

/// Native file side of the paired HTTP transport. Paths are private staging files chosen by the
/// caller, never names received from the server. The caller discards a partial file on failure.
pub trait FileTransport {
    fn download_to(
        &self,
        id: AttachmentId,
        size: u64,
        path: &Path,
    ) -> impl Future<Output = Result<(), FileError>> + Send;
    fn upload_from(
        &self,
        task: TaskId,
        id: AttachmentId,
        name: &str,
        media: &str,
        path: &Path,
    ) -> impl Future<Output = Result<(), FileError>> + Send;
}

impl FileTransport for HttpTransport {
    async fn download_to(&self, id: AttachmentId, size: u64, path: &Path) -> Result<(), FileError> {
        let mut res = self
            .authenticated(Method::Get, &format!("/api/v1/attachments/{id}"))?
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(network)?;
        if res.status().as_u16() != 200 {
            return Err(FileError::Refused {
                status: res.status().as_u16(),
                max_bytes: None,
            });
        }
        if res.content_length().is_some_and(|n| n != size) {
            return Err(FileError::WrongSize);
        }
        let mut file = File::options()
            .write(true)
            .truncate(true)
            .open(path)
            .await?;
        let mut remaining = size;
        while let Some(chunk) = res.chunk().await.map_err(network)? {
            remaining = remaining
                .checked_sub(chunk.len() as u64)
                .ok_or(FileError::WrongSize)?;
            file.write_all(&chunk).await?;
        }
        if remaining != 0 {
            return Err(FileError::WrongSize);
        }
        file.flush().await?;
        Ok(())
    }

    async fn upload_from(
        &self,
        task: TaskId,
        id: AttachmentId,
        name: &str,
        media: &str,
        path: &Path,
    ) -> Result<(), FileError> {
        let file = File::open(path).await?;
        let size = file.metadata().await?.len();
        if size > MAX_BYTES {
            return Err(FileError::Refused {
                status: 413,
                max_bytes: Some(MAX_BYTES),
            });
        }
        let path = attachment_upload_path(task, id, name);
        let mut res = self
            .authenticated(Method::Post, &path)?
            .header(reqwest::header::CONTENT_TYPE, media)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .body(file)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(network)?;
        let status = res.status().as_u16();
        if matches!(status, 200 | 201) {
            return Ok(());
        }
        let mut max_bytes = None;
        if status == 413 {
            let mut body = Vec::new();
            while let Ok(Some(chunk)) = res.chunk().await {
                if body.len() + chunk.len() > 8192 {
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            max_bytes = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|json| json.pointer("/error/details/max_bytes")?.as_u64());
        }
        Err(FileError::Refused { status, max_bytes })
    }
}
