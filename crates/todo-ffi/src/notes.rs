//! The shared notes document at the UniFFI boundary. The client only styles it; parsing and
//! deciding which destinations are links stay in `todo_client::notes_markdown`.

use todo_client::notes_markdown::{self, Block, ListItem, Span, TaskMarker};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NoteDocument {
    pub blocks: Vec<NoteBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NoteBlock {
    Paragraph {
        spans: Vec<NoteSpan>,
    },
    Heading {
        level: u8,
        spans: Vec<NoteSpan>,
    },
    ListBlock {
        start: Option<u64>,
        items: Vec<NoteListItem>,
    },
    Quote {
        blocks: Vec<NoteBlock>,
    },
    Code {
        text: String,
    },
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NoteListItem {
    pub checkbox: Option<NoteCheckbox>,
    pub blocks: Vec<NoteBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NoteCheckbox {
    /// Byte offset in the exact source this document was parsed from, not a Kotlin string index.
    pub offset: u32,
    pub checked: bool,
    pub label: String,
}

impl From<NoteCheckbox> for TaskMarker {
    fn from(value: NoteCheckbox) -> Self {
        Self {
            offset: value.offset,
            checked: value.checked,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NoteSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// The shared parser supplies only absolute HTTP, HTTPS and mailto destinations.
    pub link: Option<String>,
}

impl From<Span> for NoteSpan {
    fn from(span: Span) -> Self {
        Self {
            text: span.text,
            bold: span.style.bold,
            italic: span.style.italic,
            strike: span.style.strike,
            code: span.style.code,
            link: span.link.map(|link| link.as_str().to_owned()),
        }
    }
}

impl From<ListItem> for NoteListItem {
    fn from(item: ListItem) -> Self {
        let checkbox = item.task.map(|task| NoteCheckbox {
            offset: task.offset,
            checked: task.checked,
            label: item.label(),
        });
        Self {
            checkbox,
            blocks: item.blocks.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<Block> for NoteBlock {
    fn from(block: Block) -> Self {
        match block {
            Block::Paragraph(spans) => Self::Paragraph {
                spans: spans.into_iter().map(Into::into).collect(),
            },
            Block::Heading { level, spans } => Self::Heading {
                level,
                spans: spans.into_iter().map(Into::into).collect(),
            },
            Block::List { start, items } => Self::ListBlock {
                start,
                items: items.into_iter().map(Into::into).collect(),
            },
            Block::Quote(blocks) => Self::Quote {
                blocks: blocks.into_iter().map(Into::into).collect(),
            },
            Block::Code(text) => Self::Code { text },
            Block::Rule => Self::Rule,
        }
    }
}

#[uniffi::export]
pub fn parse_note(source: String) -> NoteDocument {
    NoteDocument {
        blocks: notes_markdown::parse(&source)
            .blocks
            .into_iter()
            .map(Into::into)
            .collect(),
    }
}
