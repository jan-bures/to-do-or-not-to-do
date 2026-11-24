//! The display form of task notes. Never stores HTML or an unvalidated link as an action.

use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
use url::Url;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    Code(String),
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub task: Option<TaskMarker>,
    pub blocks: Vec<Block>,
}

impl ListItem {
    /// An accessible name for a task-list item's checkbox (not its current state).
    pub fn label(&self) -> String {
        if let Some(Block::Paragraph(spans) | Block::Heading { spans, .. }) = self.blocks.first() {
            let text: String = spans.iter().map(|span| span.text.as_str()).collect();
            if !text.trim().is_empty() {
                return text;
            }
        }
        "Checklist item".into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskMarker {
    /// UTF-8 byte offset of the opening bracket in the original source.
    pub offset: u32,
    pub checked: bool,
}

impl TaskMarker {
    /// Toggle just this marker. Callers must supply the source this document was parsed from,
    /// not replace a note that has since changed on another client.
    pub fn toggle(self, source: &str) -> Option<String> {
        let start = self.offset as usize;
        let end = start.checked_add(3)?;
        let replacement = match (source.get(start..end)?, self.checked) {
            ("[x]" | "[X]", true) => "[ ]",
            ("[ ]" | "[\t]" | "[\x0c]" | "[\r]", false) => "[x]",
            _ => return None,
        };
        let mut result = source.to_owned();
        result.replace_range(start..end, replacement);
        Some(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub link: Option<SafeLink>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// An absolute destination known to use HTTP, HTTPS or mailto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeLink(String);

impl SafeLink {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(text: &str) -> Option<Self> {
        let (scheme, rest) = text.split_once(':')?;
        if text.chars().any(char::is_control)
            || !(scheme.eq_ignore_ascii_case("mailto")
                || ((scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
                    && rest.starts_with("//")))
        {
            return None;
        }
        let url = Url::parse(text).ok()?;
        match url.scheme() {
            "http" | "https" if url.has_host() => Some(Self(url.into())),
            "mailto" if !url.has_host() && !url.path().is_empty() => Some(Self(url.into())),
            _ => None,
        }
    }
}

// Limit render-tree depth, even for arbitrary input. Additional levels keep all their content
// but lose the extra indentation. The parser itself keeps its stack on the heap.
const MAX_LIST_DEPTH: usize = 3;
const MAX_QUOTE_DEPTH: usize = 3;

#[derive(Default)]
enum Kind {
    #[default]
    Flow,
    Heading(u8),
    Quote(bool),
    List(Option<u64>, Option<Vec<ListItem>>),
    Item(Option<TaskMarker>),
    Code,
    Html,
}

#[derive(Default)]
struct Frame {
    kind: Kind,
    blocks: Vec<Block>,
    spans: Vec<Span>,
    autolink: Vec<bool>,
}

impl Frame {
    fn push(&mut self, span: Span, autolink: bool) {
        if span.text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.style == span.style
            && last.link == span.link
            && self.autolink.last() == Some(&autolink)
        {
            last.text.push_str(&span.text);
        } else {
            self.spans.push(span);
            self.autolink.push(autolink);
        }
    }

    fn take_spans(&mut self, finder: &LinkFinder) -> Vec<Span> {
        std::mem::take(&mut self.spans)
            .into_iter()
            .zip(std::mem::take(&mut self.autolink))
            .flat_map(|(span, auto)| {
                if !auto {
                    return vec![span];
                }
                let mut parts = Vec::new();
                let mut start = 0;
                // Autolink across adjacent parser text events (e.g. a URL with an entity).
                for link in finder.links(&span.text) {
                    let Some(target) = SafeLink::parse(link.as_str()) else {
                        continue;
                    };
                    if start < link.start() {
                        parts.push(Span {
                            text: span.text[start..link.start()].into(),
                            style: span.style,
                            link: None,
                        });
                    }
                    parts.push(Span {
                        text: link.as_str().into(),
                        style: span.style,
                        link: Some(target),
                    });
                    start = link.end();
                }
                if start == 0 {
                    return vec![span];
                }
                if start < span.text.len() {
                    parts.push(Span {
                        text: span.text[start..].into(),
                        style: span.style,
                        link: None,
                    });
                }
                parts
            })
            .collect()
    }

    fn flush(&mut self, finder: &LinkFinder) {
        if !self.spans.is_empty() {
            let spans = self.take_spans(finder);
            self.blocks.push(Block::Paragraph(spans));
        }
    }
}

/// Parse a note without changing its source. A soft line break in notes remains a line break.
pub fn parse(source: &str) -> Document {
    let events = Parser::new_ext(
        source,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    )
    .into_offset_iter();
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    let mut root = Frame::default();
    let mut stack: Vec<Frame> = Vec::new();
    let (mut lists, mut quotes, mut images) = (0usize, 0usize, 0usize);
    let (mut bold, mut italic, mut strike) = (0usize, 0usize, 0usize);
    let mut link: Option<Option<SafeLink>> = None;

    for (event, range) in events {
        if images > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => images += 1,
                Event::End(TagEnd::Image) => images -= 1,
                _ => {}
            }
            continue;
        }
        if let Event::TaskListMarker(checked) = event
            // Pulldown's range can include padding before the three-byte marker.
            && let Some(start) = range.end.checked_sub(3)
            && let Ok(offset) = u32::try_from(start)
        {
            if let Some(item) = stack
                .iter_mut()
                .rev()
                .find_map(|frame| match &mut frame.kind {
                    Kind::Item(task) => Some(task),
                    _ => None,
                })
            {
                *item = Some(TaskMarker { offset, checked });
            }
            continue;
        }
        let frame = stack.last_mut().unwrap_or(&mut root);
        let style = Style {
            bold: bold > 0,
            italic: italic > 0,
            strike: strike > 0,
            code: false,
        };
        let add = |frame: &mut Frame, text: &str, code: bool, auto: bool| {
            frame.push(
                Span {
                    text: text.into(),
                    style: Style { code, ..style },
                    link: link.clone().flatten(),
                },
                auto && link.is_none(),
            )
        };
        let start = match event {
            Event::Start(Tag::Paragraph) => Some(Kind::Flow),
            Event::Start(Tag::Heading { level, .. }) => Some(Kind::Heading(match level {
                HeadingLevel::H1 => 1,
                HeadingLevel::H2 => 2,
                _ => 3,
            })),
            Event::Start(Tag::List(start)) => {
                lists += 1;
                Some(Kind::List(start, (lists <= MAX_LIST_DEPTH).then(Vec::new)))
            }
            Event::Start(Tag::Item) => Some(Kind::Item(None)),
            Event::Start(Tag::BlockQuote(_)) => {
                quotes += 1;
                Some(Kind::Quote(quotes <= MAX_QUOTE_DEPTH))
            }
            Event::Start(Tag::CodeBlock(_)) => Some(Kind::Code),
            Event::Start(Tag::HtmlBlock) => Some(Kind::Html),
            Event::Start(Tag::Strong) => {
                bold += 1;
                None
            }
            Event::Start(Tag::Emphasis) => {
                italic += 1;
                None
            }
            Event::Start(Tag::Strikethrough) => {
                strike += 1;
                None
            }
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) => {
                link = Some(if link_type == LinkType::Email {
                    SafeLink::parse(&format!("mailto:{dest_url}"))
                } else {
                    SafeLink::parse(&dest_url)
                });
                None
            }
            Event::Start(Tag::Image { .. }) => {
                // Preserve the complete source, including markup and destination. Never autolink it.
                frame.push(
                    Span {
                        text: source.get(range).unwrap_or_default().into(),
                        style,
                        link: None,
                    },
                    false,
                );
                images = 1;
                None
            }
            Event::End(TagEnd::Strong) => {
                bold -= 1;
                None
            }
            Event::End(TagEnd::Emphasis) => {
                italic -= 1;
                None
            }
            Event::End(TagEnd::Strikethrough) => {
                strike -= 1;
                None
            }
            Event::End(TagEnd::Link) => {
                link = None;
                None
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::List(_)
                | TagEnd::Item
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::HtmlBlock,
            ) => {
                if let Some(mut frame) = stack.pop() {
                    let parent = stack.last_mut().unwrap_or(&mut root);
                    match frame.kind {
                        Kind::Heading(level) => parent.blocks.push(Block::Heading {
                            level,
                            spans: frame.take_spans(&finder),
                        }),
                        Kind::Code => {
                            let mut text: String =
                                frame.spans.into_iter().map(|span| span.text).collect();
                            if text.ends_with('\n') {
                                text.pop();
                            }
                            parent.blocks.push(Block::Code(text));
                        }
                        Kind::List(start, Some(items)) => {
                            parent.blocks.push(Block::List { start, items })
                        }
                        Kind::List(_, None) => parent.blocks.extend(frame.blocks),
                        Kind::Item(task) => {
                            frame.flush(&finder);
                            if let Kind::List(_, Some(items)) = &mut parent.kind {
                                items.push(ListItem {
                                    task,
                                    blocks: frame.blocks,
                                });
                            } else {
                                if let Some(task) = task {
                                    let marker = if task.checked { "[x]" } else { "[ ]" };
                                    let span = Span {
                                        text: format!("{marker} "),
                                        style: Style::default(),
                                        link: None,
                                    };
                                    if let Some(Block::Paragraph(spans)) = frame.blocks.first_mut()
                                    {
                                        spans.insert(0, span);
                                    } else {
                                        frame.blocks.insert(0, Block::Paragraph(vec![span]));
                                    }
                                }
                                parent.blocks.extend(frame.blocks);
                            }
                        }
                        Kind::Quote(true) => {
                            frame.flush(&finder);
                            parent.blocks.push(Block::Quote(frame.blocks));
                        }
                        Kind::Quote(false) | Kind::Flow | Kind::Html => {
                            frame.flush(&finder);
                            parent.blocks.extend(frame.blocks);
                        }
                    }
                }
                match event {
                    Event::End(TagEnd::List(_)) => lists -= 1,
                    Event::End(TagEnd::BlockQuote(_)) => quotes -= 1,
                    _ => {}
                }
                None
            }
            Event::Text(text) => {
                add(frame, &text, false, !matches!(frame.kind, Kind::Code));
                None
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                add(frame, &text, false, false);
                None
            }
            Event::Code(text) => {
                add(frame, &text, true, false);
                None
            }
            Event::SoftBreak | Event::HardBreak => {
                add(frame, "\n", false, false);
                None
            }
            Event::Rule => {
                frame.flush(&finder);
                frame.blocks.push(Block::Rule);
                None
            }
            // Other extensions aren't enabled.
            _ => None,
        };
        if let Some(kind) = start {
            stack.last_mut().unwrap_or(&mut root).flush(&finder);
            stack.push(Frame {
                kind,
                ..Frame::default()
            });
        }
    }
    root.flush(&finder);
    Document {
        blocks: root.blocks,
    }
}

impl Document {
    /// Visible text without presentation (bullets, numbers and checkbox glyphs are not text).
    pub fn plain_text(&self) -> String {
        fn append(out: &mut String, blocks: &[Block]) {
            for block in blocks {
                match block {
                    Block::Paragraph(spans) | Block::Heading { spans, .. } => {
                        for span in spans {
                            out.push_str(&span.text);
                        }
                        out.push('\n');
                    }
                    Block::List { items, .. } => {
                        for item in items {
                            append(out, &item.blocks);
                        }
                    }
                    Block::Quote(blocks) => append(out, blocks),
                    Block::Code(text) => {
                        out.push_str(text);
                        out.push('\n');
                    }
                    Block::Rule => out.push('\n'),
                }
            }
        }
        let mut text = String::new();
        append(&mut text, &self.blocks);
        if text.ends_with('\n') {
            text.pop();
        }
        text
    }
}
