//! Direct, bounded file transfers. Neither the outbox nor the JVM heap holds the content.
use super::HttpTransport;
use crate::failure;
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
    #[error("could not connect")]
    Connect,
    #[error("the transfer timed out")]
    Timeout,
    #[error("connection interrupted during the transfer")]
    Interrupted,
    #[error("could not read or write the file")]
    File(#[from] std::io::Error),
    #[error("the file transfer was refused (HTTP {status})")]
    Refused {
        status: u16,
        max_bytes: Option<u64>,
        code: Option<String>,
    },
    #[error("the download size did not match the attachment")]
    WrongSize,
}

fn transfer_error(e: reqwest::Error) -> FileError {
    if e.is_connect() {
        FileError::Connect
    } else if e.is_timeout() {
        FileError::Timeout
    } else {
        FileError::Interrupted
    }
}

impl FileError {
    pub fn download_message(&self) -> &'static str {
        match self {
            Self::Timeout => failure::DOWNLOAD_TIMEOUT,
            Self::Connect => failure::ATTACHMENT_OFFLINE,
            Self::WrongSize => "The download didn’t match the attachment size. Try again.",
            Self::Refused { status, code, .. } => {
                failure::attachment_download(*status, code.as_deref())
            }
            _ => "Couldn’t download the attachment.",
        }
    }

    pub fn upload_message(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Connect => failure::ATTACHMENT_OFFLINE.into(),
            Self::Timeout => "The upload timed out. Check the task before trying again.".into(),
            Self::Interrupted => {
                "Couldn’t upload the file. The connection dropped, or the server refused it.".into()
            }
            Self::Refused { status: 401, .. } => {
                "This device is signed out. Pair again to upload attachments.".into()
            }
            Self::Refused { status: 404, .. } => {
                "Couldn’t upload the file. The task is no longer available.".into()
            }
            Self::Refused {
                status: status @ (413 | 429 | 507),
                max_bytes,
                ..
            } => failure::attachment_upload(Some(*status), *max_bytes),
            _ => "Couldn’t upload the selected file.".into(),
        }
    }
}

// A refusal may arrive before the upload body is consumed; never infer offline from that.
async fn refused(mut res: reqwest::Response) -> FileError {
    let status = res.status().as_u16();
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = res.chunk().await {
        if body.len() + chunk.len() > 8192 {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let error = serde_json::from_slice::<crate::protocol::ErrorBody>(&body)
        .ok()
        .map(|e| e.error);
    let max_bytes = error
        .as_ref()
        .and_then(|e| e.details.as_ref()?.get("max_bytes")?.as_u64());
    FileError::Refused {
        status,
        max_bytes,
        code: error.map(|e| e.code),
    }
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

impl HttpTransport {
    async fn attachment_response(&self, id: AttachmentId) -> Result<reqwest::Response, FileError> {
        let res = self
            .authenticated(Method::Get, &format!("/api/v1/attachments/{id}"))?
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(transfer_error)?;
        if res.status().as_u16() != 200 {
            return Err(refused(res).await);
        }
        Ok(res)
    }

    /// Desktop buffers the download before writing and marking it in a single filesystem operation.
    pub async fn download_attachment_bytes(&self, id: AttachmentId) -> Result<Vec<u8>, FileError> {
        self.attachment_response(id)
            .await?
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(transfer_error)
    }
}

impl FileTransport for HttpTransport {
    async fn download_to(&self, id: AttachmentId, size: u64, path: &Path) -> Result<(), FileError> {
        let mut res = self.attachment_response(id).await?;
        if res.content_length().is_some_and(|n| n != size) {
            return Err(FileError::WrongSize);
        }
        let mut file = File::options()
            .write(true)
            .truncate(true)
            .open(path)
            .await?;
        let mut remaining = size;
        while let Some(chunk) = res.chunk().await.map_err(transfer_error)? {
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
                code: None,
            });
        }
        let path = attachment_upload_path(task, id, name);
        let res = self
            .authenticated(Method::Post, &path)?
            .header(reqwest::header::CONTENT_TYPE, media)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .body(file)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(transfer_error)?;
        let status = res.status().as_u16();
        if matches!(status, 200 | 201) {
            return Ok(());
        }
        Err(refused(res).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_timeout_words_the_failure_as_offline() {
        // Accept TCP, but never answer the TLS handshake. The connector times out on loopback
        // without relying on how the host network handles unroutable addresses.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let error = client
            .get(format!("https://{address}"))
            .send()
            .await
            .unwrap_err();
        server.abort();
        assert!(error.is_connect() && error.is_timeout(), "{error:?}");
        let error = transfer_error(error);
        assert_eq!(error.upload_message(), failure::ATTACHMENT_OFFLINE);
        assert_eq!(error.download_message(), failure::ATTACHMENT_OFFLINE);
    }
}
