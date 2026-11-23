//! Turn a dropped RFC 5322 message into ordinary task commands. No file I/O, HTML rendering,
//! URL discovery, or network access here. Both clients put the same bounds on the file read.

use mail_parser::{DecodeProblems, MessageParser, PartKind, html_to_text};
use todo_core::decide::{MAX_TITLE, MAX_URL, validate_link};
use todo_core::ids::LinkId;
use todo_core::model::{EmailLink, LinkKind};
use todo_core::time::{Date, Timestamp};
use todo_core::{Command, State};
use uuid::Uuid;

use super::{TaskRoom, add_task, quick_add::Parsed};

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_FILES: usize = 20;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_EXCERPT_BYTES: usize = 4 * 1024;
pub const DROP_LABEL: &str = "Drop .eml files to create tasks";

pub fn is_eml_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("eml")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("Couldn't read the email file.")]
    Read,
    #[error("Email files must be 10 MiB or smaller.")]
    TooLarge,
    #[error("Couldn't parse the email file.")]
    Invalid,
    #[error("Drop up to 20 email files at a time.")]
    TooMany,
    #[error("Couldn't save a task from the email.")]
    Save,
    #[error("The destination is no longer available.")]
    Destination,
}

/// One toast per gesture. Partial success keeps Undo available without hiding the failures.
#[derive(Debug, Default)]
pub struct Feedback {
    pub created: usize,
    failed: usize,
    first_error: Option<ImportError>,
}

impl Feedback {
    pub fn failed(&mut self, error: ImportError) {
        self.failed += 1;
        if self.first_error.is_none() {
            self.first_error = Some(error);
        }
    }

    pub fn message(&self) -> Option<String> {
        Some(match (self.created, self.failed) {
            (0, 0) => return None,
            (0, 1) => self.first_error?.to_string(),
            (0, failed) => format!("{} ({failed} issues)", self.first_error?),
            (1, 0) => "Created 1 email task".into(),
            (created, 0) => format!("Created {created} email tasks"),
            (created, failed) => {
                let noun = if created == 1 { "task" } else { "tasks" };
                format!("Created {created} email {noun} · {failed} failed")
            }
        })
    }
}

/// A file can finish reading after its originating room has been deleted. Do not silently
/// redirect that drop to another list.
pub fn destination_available(state: &State, room: TaskRoom) -> bool {
    match room {
        TaskRoom::List(id) => state.is_live(id.into()),
        TaskRoom::Collection(id) => state.is_live(id.into()),
        _ => true,
    }
}

/// Queue one message in order. An automatic destination list is kept by Undo; removing it might
/// hide unrelated tasks. If the link can't be queued, the successful task is still undoable.
pub fn submit_message<T>(
    commands: Vec<Command>,
    feedback: &mut Feedback,
    undo: &mut Vec<T>,
    mut submit: impl FnMut(Command) -> Result<T, ()>,
) {
    for cmd in commands {
        let is_list = matches!(cmd, Command::CreateList { .. });
        let is_task = matches!(cmd, Command::CreateTask(_));
        match submit(cmd) {
            Ok(target) if !is_list => {
                feedback.created += usize::from(is_task);
                undo.push(target);
            }
            Ok(_) => (),
            Err(()) => {
                feedback.failed(ImportError::Save);
                break;
            }
        }
    }
}

pub struct DroppedEmail {
    title: String,
    notes: String,
    link: Option<EmailLink>,
}

// Do not put message content in traces, even via a derived Debug implementation.
impl std::fmt::Debug for DroppedEmail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DroppedEmail").finish_non_exhaustive()
    }
}

fn one_line(s: &str, max_chars: usize) -> String {
    s.split(|ch: char| ch.is_whitespace() || ch.is_control())
        .filter(|word| !word.is_empty())
        .enumerate()
        .flat_map(|(i, word)| (i > 0).then_some(' ').into_iter().chain(word.chars()))
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

// The same prefixes and whole-word #keep marker as the forwarding contract. A drop already
// explicitly asks to capture an excerpt, so the marker affects only the title here.
fn strip_prefixes(mut subject: &str) -> &str {
    while let Some((prefix, rest)) = subject.split_once([':', '：']) {
        let prefix = prefix.trim_end();
        let prefix = if let Some((base, count)) = prefix.split_once(['[', '(']) {
            let closing = if prefix.contains('[') { ']' } else { ')' };
            if !count
                .strip_suffix(closing)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                break;
            }
            base.trim_end()
        } else {
            prefix
        };
        if !["fw", "fwd", "wg", "re", "aw"]
            .iter()
            .any(|p| p.eq_ignore_ascii_case(prefix))
        {
            break;
        }
        subject = rest.trim_start();
    }
    subject
}

fn subject_title(subject: &str) -> String {
    let words = strip_prefixes(subject)
        .split_whitespace()
        .filter(|word| {
            !word
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']'])
                .eq_ignore_ascii_case("#keep")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let title = one_line(strip_prefixes(&words), MAX_TITLE);
    if title.is_empty() {
        "(no subject)".into()
    } else {
        title
    }
}

fn byte_prefix(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

fn body_text(text: &str, html: bool) -> String {
    let text = if html {
        std::borrow::Cow::Owned(html_to_text(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    // Collect just enough to detect truncation, then leave room for an ellipsis.
    for ch in text
        .trim()
        .chars()
        .filter(|&c| !c.is_control() || matches!(c, '\t' | '\n'))
    {
        if out.len() + ch.len_utf8() > MAX_EXCERPT_BYTES {
            out.truncate(out.floor_char_boundary(MAX_EXCERPT_BYTES - '…'.len_utf8()));
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.trim().to_owned()
}

/// The first nonblank plain part wins over HTML. Never read from an attachment or nested message.
fn excerpt(message: &mail_parser::Message<'_>) -> Result<String, ImportError> {
    let plain = message
        .text_body()
        .filter(|part| matches!(part.kind(), PartKind::Text));
    let html = message
        .html_body()
        .filter(|part| matches!(part.kind(), PartKind::Html));
    for (part, html) in plain
        .zip(std::iter::repeat(false))
        .chain(html.zip(std::iter::repeat(true)))
    {
        if part.is_attachment() {
            continue;
        }
        let (text, problems) = part.text_checked().ok_or(ImportError::Invalid)?;
        if problems.contains(DecodeProblems::MALFORMED_TRANSFER_ENCODING) {
            return Err(ImportError::Invalid);
        }
        let text = body_text(&text, html);
        if !text.is_empty() {
            return Ok(text);
        }
    }
    Ok(String::new())
}

pub fn parse(raw: &[u8], imported_at: Timestamp) -> Result<DroppedEmail, ImportError> {
    if raw.len() > MAX_FILE_BYTES {
        return Err(ImportError::TooLarge);
    }
    let message = MessageParser::default()
        .max_depth(16)
        .max_parts(100)
        .parse(raw)
        .ok_or(ImportError::Invalid)?;
    let root = message.root_part();
    // The parser is forgiving by design. Don't turn an arbitrary text file renamed to .eml into
    // a task, or silently import part of a structurally damaged MIME message.
    if root.raw_headers().len() > MAX_HEADER_BYTES
        || message.root().parts().any(|part| part.has_problems())
        || !(message.subject().is_some()
            || message.from().is_some()
            || message.date().is_some()
            || message.message_id().is_some()
            || message.content_type().is_some())
    {
        return Err(ImportError::Invalid);
    }
    let subject = one_line(message.subject().unwrap_or_default(), MAX_HEADER_BYTES);
    let title = subject_title(&subject);
    let subject = one_line(&subject, 998);
    let sender = message.from().and_then(|list| list.first());
    let address = sender
        .and_then(|sender| sender.address())
        .filter(|address| {
            !address.is_empty() && address.len() <= MAX_URL && !address.contains(char::is_control)
        });
    let name = one_line(
        sender.and_then(|sender| sender.name()).unwrap_or_default(),
        200,
    );
    let mut notes = match (name.is_empty(), address) {
        (false, Some(addr)) if name != addr => format!("From: {name} <{addr}>"),
        (_, Some(addr)) => format!("From: {addr}"),
        (false, None) => format!("From: {name}"),
        (true, None) => String::new(),
    };
    let body = excerpt(&message)?;
    if !body.is_empty() {
        if !notes.is_empty() {
            notes.push_str("\n\n");
        }
        notes.push_str(&body);
    }
    let link = address.zip(message.message_id()).and_then(|(sender, id)| {
        let (local, domain) = id.rsplit_once('@')?;
        if local.is_empty()
            || domain.is_empty()
            || id.len() > 996
            || id.contains(|ch: char| {
                ch.is_whitespace() || ch.is_control() || matches!(ch, '<' | '>')
            })
        {
            return None;
        }
        let kind = LinkKind::Email(EmailLink {
            subject: byte_prefix(if subject.is_empty() { &title } else { &subject }, MAX_URL)
                .into(),
            sender: sender.into(),
            received_at: message
                .received()
                .and_then(|h| h.date())
                .or_else(|| message.date())
                .and_then(|date| Timestamp::from_second(date.to_timestamp()).ok())
                .unwrap_or(imported_at),
            message_id: format!("<{id}>"),
            // Like server ingest, keep the ID for mail clients, not a guessed URL or a link
            // found in the body (which could point anywhere).
            web_link: None,
        });
        validate_link(&kind).ok()?;
        match kind {
            LinkKind::Email(link) => Some(link),
            _ => None,
        }
    });
    Ok(DroppedEmail { title, notes, link })
}

impl DroppedEmail {
    /// Use quick-add's destination, room defaults and positioning, but never interpret a subject
    /// suffix as a date or priority. The caller submits these in order and tracks task/link undo.
    pub fn commands(
        self,
        state: &State,
        room: impl Into<TaskRoom>,
        today: Date,
        mut new_id: impl FnMut() -> Uuid,
    ) -> Vec<Command> {
        let (task, mut cmds) = add_task(
            state,
            room,
            &Parsed {
                title: self.title,
                due: None,
                priority: None,
            },
            today,
            &mut new_id,
        );
        if let Some(Command::CreateTask(task)) = cmds.last_mut() {
            task.notes = self.notes;
        }
        if let Some((task, link)) = task.zip(self.link) {
            cmds.push(Command::AddLink {
                id: LinkId::from_uuid(new_id()),
                task,
                kind: LinkKind::Email(link),
            });
        }
        cmds
    }
}
