//! An incremental Server-Sent Events parser, for transports that read the stream as raw bytes.
//!
//! Follows the WHATWG event-stream format: lines end in LF, CRLF or CR; a blank line dispatches the
//! frame; `data:` lines are joined with LF; a line starting with `:` is a comment; one space after the
//! colon is stripped; unknown fields are ignored.

use crate::platform::SseFrame;

/// Longest line accepted before the buffer is discarded (protects against a stream without newlines).
const MAX_LINE: usize = 8 * 1024 * 1024;

/// Turns chunks of an event stream into [`SseFrame`]s.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    /// The previous chunk ended in CR, so a leading LF belongs to that line ending.
    after_cr: bool,
    event: String,
    data: String,
    has_data: bool,
    id: Option<String>,
}

impl SseParser {
    /// A parser at the start of a stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk and returns the frames it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
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
                    if self.line.len() >= MAX_LINE {
                        self.line.clear();
                    }
                    self.line.push(b);
                }
            }
        }
        out
    }

    fn end_line(&mut self, out: &mut Vec<SseFrame>) {
        let line = std::mem::take(&mut self.line);
        let line = String::from_utf8_lossy(&line);
        if line.is_empty() {
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
        let mut frames = p.push(b":heartbeat\n\nid: 1\nevent: ev");
        frames.extend(p.push(b"ent\ndata: {\"a\":\ndata: 1}\n"));
        frames.extend(p.push(b"\n"));
        assert_eq!(
            frames,
            vec![SseFrame::Comment, ev("event", Some("1"), "{\"a\":\n1}")]
        );
    }

    #[test]
    fn handles_crlf_and_cr_line_endings() {
        let mut p = SseParser::new();
        let mut frames = p.push(b"data: a\r");
        frames.extend(p.push(b"\n\r\ndata:b\r\r"));
        assert_eq!(
            frames,
            vec![ev("message", None, "a"), ev("message", None, "b")]
        );
    }

    #[test]
    fn a_frame_without_data_is_not_dispatched() {
        let mut p = SseParser::new();
        assert!(p.push(b"event: x\n\n").is_empty());
        assert_eq!(p.push(b"data\n\n"), vec![ev("message", None, "")]);
    }
}
