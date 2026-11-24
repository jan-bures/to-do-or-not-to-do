//! A small, text-only Markdown dialect for untrusted assistant replies.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownBlock {
    Heading { level: u8, spans: Vec<MarkdownSpan> },
    Paragraph(Vec<MarkdownSpan>),
    UnorderedList(Vec<Vec<MarkdownSpan>>),
    OrderedList(Vec<OrderedItem>),
    Code(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedItem {
    pub number: u32,
    pub spans: Vec<MarkdownSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownSpan {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
}

/// Parse the reply so far. A fence takes effect once its closing line arrives.
pub fn parse(source: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut lines = source.lines().peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if fence(line) {
            let mut code = Vec::new();
            loop {
                match lines.next() {
                    Some(close) if close.trim() == "```" => {
                        blocks.push(MarkdownBlock::Code(code.join("\n")));
                        break;
                    }
                    Some(content) => code.push(content),
                    None => {
                        blocks.push(MarkdownBlock::Paragraph(vec![MarkdownSpan::Text(
                            std::iter::once(raw)
                                .chain(code)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        )]));
                        break;
                    }
                }
            }
        } else if let Some((level, body)) = heading(line) {
            blocks.push(MarkdownBlock::Heading {
                level,
                spans: inline(body),
            });
        } else if let Some(item) = bullet(line) {
            let mut items = vec![inline(item)];
            while let Some(item) = lines.peek().and_then(|line| bullet(line.trim())) {
                items.push(inline(item));
                lines.next();
            }
            blocks.push(MarkdownBlock::UnorderedList(items));
        } else if let Some((number, item)) = ordered(line) {
            let mut items = vec![OrderedItem {
                number,
                spans: inline(item),
            }];
            while let Some((number, item)) = lines.peek().and_then(|line| ordered(line.trim())) {
                items.push(OrderedItem {
                    number,
                    spans: inline(item),
                });
                lines.next();
            }
            blocks.push(MarkdownBlock::OrderedList(items));
        } else {
            let mut paragraph = line.to_owned();
            while let Some(next) = lines.peek().map(|line| line.trim()) {
                if next.is_empty()
                    || fence(next)
                    || heading(next).is_some()
                    || bullet(next).is_some()
                    || ordered(next).is_some()
                {
                    break;
                }
                paragraph.push(' ');
                paragraph.push_str(next);
                lines.next();
            }
            blocks.push(MarkdownBlock::Paragraph(inline(&paragraph)));
        }
    }
    blocks
}

fn fence(line: &str) -> bool {
    line.strip_prefix("```")
        .is_some_and(|info| !info.contains('`'))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let (prefix, body) = line.split_once(' ')?;
    let level = match prefix {
        "#" => 1,
        "##" => 2,
        "###" => 3,
        _ => return None,
    };
    Some((level, body.trim_start()))
}

fn bullet(line: &str) -> Option<&str> {
    ["- ", "* ", "• "]
        .into_iter()
        .find_map(|marker| line.strip_prefix(marker).map(str::trim_start))
}

fn ordered(line: &str) -> Option<(u32, &str)> {
    let (prefix, body) = line.split_once(". ")?;
    if !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((prefix.parse().ok()?, body.trim_start()))
}

fn inline(source: &str) -> Vec<MarkdownSpan> {
    let delimiters: Vec<_> = delimiter_runs(source).collect();
    // Index the next eligible closer for each marker in one pass. A reply with
    // many unmatched delimiters should take the same time as ordinary prose.
    const MARKERS: [&str; 5] = ["**", "__", "*", "_", "`"];
    let mut next = [None; MARKERS.len()];
    let mut pairs = vec![None; delimiters.len()];
    for (index, &(start, marker)) in delimiters.iter().enumerate().rev() {
        let Some(kind) = MARKERS.iter().position(|&known| marker == known) else {
            continue;
        };
        let (before, after) = (&source[..start], &source[start + marker.len()..]);
        let underscore = marker.starts_with('_');
        let code = marker == "`";
        if code
            || (!after.starts_with(char::is_whitespace)
                && !(underscore && before.ends_with(char::is_alphanumeric)))
        {
            pairs[index] = next[kind];
        }
        if code
            || (!before.ends_with(char::is_whitespace)
                && !(underscore && after.starts_with(char::is_alphanumeric)))
        {
            next[kind] = Some(index);
        }
    }

    let mut spans = Vec::new();
    let mut plain = 0;
    let mut index = 0;
    while let Some(&(start, marker)) = delimiters.get(index) {
        let Some(close) = pairs[index] else {
            index += 1;
            continue;
        };
        if start > plain {
            spans.push(MarkdownSpan::Text(source[plain..start].into()));
        }
        let end = delimiters[close].0;
        let text = source[start + marker.len()..end].into();
        spans.push(match marker {
            "**" | "__" => MarkdownSpan::Bold(text),
            "*" | "_" => MarkdownSpan::Italic(text),
            _ => MarkdownSpan::Code(text),
        });
        plain = end + marker.len();
        index = close + 1;
    }
    if plain < source.len() {
        spans.push(MarkdownSpan::Text(source[plain..].into()));
    }
    spans
}

fn delimiter_runs(mut rest: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        let start = rest.find(['*', '_', '`'])?;
        let marker = rest.as_bytes()[start];
        let len = rest.as_bytes()[start..]
            .iter()
            .take_while(|&&byte| byte == marker)
            .count();
        let span = (offset + start, &rest[start..start + len]);
        offset += start + len;
        rest = &rest[start + len..];
        Some(span)
    })
}

#[cfg(test)]
mod tests {
    use super::{MarkdownBlock::*, MarkdownSpan::*, OrderedItem, parse};

    #[test]
    fn parses_blocks_and_styles() {
        let source = "# First\n## Second\n### Third\n\nOne\nline with **strong**, __bold__, *soft*, _quiet_, and `snake_case`.\n\n- One\n* Two\n• Three\n\n2. A\n5. B\n\n```text\n  save()\n\n```";
        assert_eq!(
            parse(source),
            vec![
                Heading {
                    level: 1,
                    spans: vec![Text("First".into())]
                },
                Heading {
                    level: 2,
                    spans: vec![Text("Second".into())]
                },
                Heading {
                    level: 3,
                    spans: vec![Text("Third".into())]
                },
                Paragraph(vec![
                    Text("One line with ".into()),
                    Bold("strong".into()),
                    Text(", ".into()),
                    Bold("bold".into()),
                    Text(", ".into()),
                    Italic("soft".into()),
                    Text(", ".into()),
                    Italic("quiet".into()),
                    Text(", and ".into()),
                    super::MarkdownSpan::Code("snake_case".into()),
                    Text(".".into()),
                ]),
                UnorderedList(vec![
                    vec![Text("One".into())],
                    vec![Text("Two".into())],
                    vec![Text("Three".into())]
                ]),
                OrderedList(vec![
                    OrderedItem {
                        number: 2,
                        spans: vec![Text("A".into())]
                    },
                    OrderedItem {
                        number: 5,
                        spans: vec![Text("B".into())]
                    },
                ]),
                super::MarkdownBlock::Code("  save()\n".into()),
            ]
        );
    }

    #[test]
    fn keeps_streaming_tails_and_untrusted_markup_as_text() {
        for source in [
            "**unfinished*",
            "__unfinished",
            "*unfinished",
            "_unfinished",
            "`unfinished",
            "```rust\n**unfinished**\n  x",
            "snake_case_and_more",
            "<script>alert(1)</script> <b>HTML</b> https://example.com/a_b [label](https://example.com)",
        ] {
            assert_eq!(parse(source), vec![Paragraph(vec![Text(source.into())])]);
        }
    }
}
