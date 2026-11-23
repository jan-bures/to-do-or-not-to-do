//! An incremental Server-Sent Events parser, for transports that read the stream as raw bytes.
//!
//! Follows the WHATWG event-stream format: lines end in LF, CRLF or CR; a blank line dispatches the
//! frame; `data:` lines are joined with LF; a line starting with `:` is a comment; one space after the
//! colon is stripped; unknown fields are ignored.

use crate::platform::SseFrame;

/// Default for the most bytes one frame may hold (all its lines together).
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// A frame grew past the parser's limit. The stream cannot be trusted any further: end it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("an event stream frame is larger than {limit} bytes")]
pub struct FrameTooLarge {
    /// The limit.
    pub limit: usize,
}

/// Turns chunks of an event stream into [`SseFrame`]s.
///
/// The bytes of the frame being read (every line of it, comments and unknown fields included) are
/// capped at `max_frame_bytes`; past that, [`push`](Self::push) fails and the parser stays failed.
#[derive(Debug)]
pub struct SseParser {
    max_frame_bytes: usize,
    /// Bytes of the current frame so far.
    frame_bytes: usize,
    failed: bool,
    line: Vec<u8>,
    /// The previous chunk ended in CR, so a leading LF belongs to that line ending.
    after_cr: bool,
    event: String,
    data: String,
    has_data: bool,
    id: Option<String>,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::with_max_frame_bytes(DEFAULT_MAX_FRAME_BYTES)
    }
}

impl SseParser {
    /// A parser at the start of a stream, with [`DEFAULT_MAX_FRAME_BYTES`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A parser that accepts frames of at most `max_frame_bytes`.
    #[must_use]
    pub fn with_max_frame_bytes(max_frame_bytes: usize) -> Self {
        Self {
            max_frame_bytes,
            frame_bytes: 0,
            failed: false,
            line: Vec::new(),
            after_cr: false,
            event: String::new(),
            data: String::new(),
            has_data: false,
            id: None,
        }
    }

    /// Feeds a chunk and returns the frames it completed, or an error once a frame is too large.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, FrameTooLarge> {
        let too_large = FrameTooLarge {
            limit: self.max_frame_bytes,
        };
        if self.failed {
            return Err(too_large);
        }
        let mut out = Vec::new();
        for &b in chunk {
            if self.after_cr {
                self.after_cr = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\n' => self.end_line(&mut out),
                b'\r' => {
                    self.after_cr = true;
                    self.end_line(&mut out);
                }
                _ => {
                    self.frame_bytes += 1;
                    if self.frame_bytes > self.max_frame_bytes {
                        self.failed = true;
                        self.line = Vec::new();
                        self.data = String::new();
                        return Err(too_large);
                    }
                    self.line.push(b);
                }
            }
        }
        Ok(out)
    }

    /// Whether there are no undispatched bytes in the current SSE frame.
    #[cfg(feature = "assistant")]
    pub(crate) fn at_frame_boundary(&self) -> bool {
        self.frame_bytes == 0 && !self.has_data
    }

    fn end_line(&mut self, out: &mut Vec<SseFrame>) {
        let line = std::mem::take(&mut self.line);
        let line = String::from_utf8_lossy(&line);
        if line.is_empty() {
            self.frame_bytes = 0;
            self.dispatch(out);
            return;
        }
        if line.starts_with(':') {
            out.push(SseFrame::Comment);
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => value.clone_into(&mut self.event),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "id" if !value.contains('\0') => self.id = Some(value.to_owned()),
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseFrame>) {
        let event = std::mem::take(&mut self.event);
        if !self.has_data {
            return;
        }
        self.has_data = false;
        out.push(SseFrame::Event {
            event: if event.is_empty() {
                "message".to_owned()
            } else {
                event
            },
            // The last event ID persists across frames, as in the browser.
            id: self.id.clone(),
            data: std::mem::take(&mut self.data),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, id: Option<&str>, data: &str) -> SseFrame {
        SseFrame::Event {
            event: event.into(),
            id: id.map(Into::into),
            data: data.into(),
        }
    }

    #[test]
    fn parses_frames_split_across_chunks() {
        let mut p = SseParser::new();
        let mut frames = p.push(b":heartbeat\n\nid: 1\nevent: ev").unwrap();
        frames.extend(p.push(b"ent\ndata: {\"a\":\ndata: 1}\n").unwrap());
        frames.extend(p.push(b"\n").unwrap());
        assert_eq!(
            frames,
            vec![SseFrame::Comment, ev("event", Some("1"), "{\"a\":\n1}")]
        );
    }

    #[test]
    fn handles_crlf_and_cr_line_endings() {
        let mut p = SseParser::new();
        let mut frames = p.push(b"data: a\r").unwrap();
        frames.extend(p.push(b"\n\r\ndata:b\r\r").unwrap());
        assert_eq!(
            frames,
            vec![ev("message", None, "a"), ev("message", None, "b")]
        );
    }

    #[test]
    fn a_frame_without_data_is_not_dispatched() {
        let mut p = SseParser::new();
        assert!(p.push(b"event: x\n\n").unwrap().is_empty());
        assert_eq!(p.push(b"data\n\n").unwrap(), vec![ev("message", None, "")]);
    }

    #[test]
    fn a_frame_is_capped_across_lines_and_chunks() {
        let mut p = SseParser::with_max_frame_bytes(32);
        // Frames up to the cap pass, and the count starts over with each frame.
        let line = format!("data: {}\n\n", "x".repeat(26));
        for _ in 0..3 {
            assert_eq!(p.push(line.as_bytes()).unwrap().len(), 1);
        }
        // Many short lines add up, even split across chunks.
        assert!(p.push(b"data: 0123456789\n").unwrap().is_empty());
        assert!(p.push(b"data: 0123456789\n").unwrap().is_empty());
        assert_eq!(p.push(b"data: 01234567"), Err(FrameTooLarge { limit: 32 }));
        // The parser stays failed.
        assert!(p.push(b"\n\n").is_err());
        // A stream without newlines is caught too.
        let mut p = SseParser::with_max_frame_bytes(1000);
        assert!(p.push(&[b'a'; 1001]).is_err());
    }
}
