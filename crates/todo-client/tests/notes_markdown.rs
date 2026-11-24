use proptest::prelude::*;
use todo_client::notes_markdown::{Block, Span, TaskMarker, parse};

fn spans(blocks: &[Block]) -> Vec<&Span> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(spans) | Block::Heading { spans, .. } => out.extend(spans),
            Block::Quote(blocks) => out.extend(spans(blocks)),
            Block::List { items, .. } => {
                for item in items {
                    out.extend(spans(&item.blocks));
                }
            }
            Block::Code(_) | Block::Rule => {}
        }
    }
    out
}

fn tasks(blocks: &[Block]) -> Vec<TaskMarker> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Quote(blocks) => out.extend(tasks(blocks)),
            Block::List { items, .. } => {
                for item in items {
                    out.extend(item.task);
                    out.extend(tasks(&item.blocks));
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn displays_supported_syntax_and_keeps_visible_text() {
    let source = "# Heading\n\n## Middle\n\n##### Small\n\nA **bold and *italic*** ~~cut~~ `x` line.\nLine two.\n\n> Quote\n\n7. Order\n   - Leaf\n     - Last\n       - Flat\n\n---\n\n```text\n  fenced\n```\n\n    indented\n";
    let doc = parse(source);
    assert_eq!(
        doc.plain_text(),
        "Heading\nMiddle\nSmall\nA bold and italic cut x line.\nLine two.\nQuote\nOrder\nLeaf\nLast\nFlat\n\n  fenced\nindented"
    );
    for (i, block) in doc.blocks[..3].iter().enumerate() {
        assert!(matches!(block, Block::Heading { level, .. } if usize::from(*level) == i + 1));
    }
    let spans = spans(&doc.blocks);
    assert!(
        spans
            .iter()
            .any(|s| s.text == "italic" && s.style.bold && s.style.italic)
    );
    assert!(spans.iter().any(|s| s.text == "cut" && s.style.strike));
    assert!(spans.iter().any(|s| s.text == "x" && s.style.code));
    assert!(matches!(doc.blocks[5], Block::List { start: Some(7), .. }));
    assert!(matches!(doc.blocks[6], Block::Rule));
    assert!(matches!(doc.blocks[7], Block::Code(ref code) if code == "  fenced"));
    assert!(matches!(doc.blocks[8], Block::Code(ref code) if code == "indented"));
    let Block::List { items, .. } = &doc.blocks[5] else {
        unreachable!()
    };
    let Block::List { items, .. } = &items[0].blocks[1] else {
        unreachable!()
    };
    let Block::List { items, .. } = &items[0].blocks[1] else {
        unreachable!()
    };
    assert!(matches!(&items[0].blocks[1], Block::Paragraph(_)));
}

#[test]
fn literal_html_and_images_do_not_create_links_or_lose_markup() {
    let source = "An ![illustration](https://example.com/image.png \"label\") and <kbd>Enter</kbd>.\n\n<div>https://example.com</div>";
    let doc = parse(source);
    assert_eq!(doc.plain_text(), source.replace("\n\n", "\n"));
    assert!(spans(&doc.blocks).iter().all(|s| s.link.is_none()));
}

#[test]
fn only_absolute_allowed_destinations_are_actions() {
    let source = "[manual](HtTp://example.com/manual) <you@example.com> [mail](mailto:you@example.com) (https://example.com/a_(b)?x=1&amp;y=2). [label](relative) [unsafe](javascript:ignored) [local](file:///notes) [data](data:text/plain,word) `https://example.com/code`";
    let doc = parse(source);
    let links: Vec<_> = spans(&doc.blocks)
        .into_iter()
        .filter_map(|s| s.link.as_ref().map(|link| (s.text.as_str(), link.as_str())))
        .collect();
    assert_eq!(
        links,
        [
            ("manual", "http://example.com/manual"),
            ("you@example.com", "mailto:you@example.com"),
            ("mail", "mailto:you@example.com"),
            (
                "https://example.com/a_(b)?x=1&y=2",
                "https://example.com/a_(b)?x=1&y=2"
            ),
        ]
    );
    assert!(
        doc.plain_text()
            .contains("label unsafe local data https://example.com/code")
    );
}

#[test]
fn toggles_exact_byte_range_not_similar_text() {
    let source = "Příprava 🧭\r\n\r\n- [ ] First `[ ]`\r\n  - [X] **Nested**\r\n> - [x] Quoted\r\n";
    let doc = parse(source);
    let markers = tasks(&doc.blocks);
    assert_eq!(markers.len(), 3);
    assert_eq!(
        markers[0].toggle(source).expect("first marker"),
        source.replacen("- [ ]", "- [x]", 1)
    );
    assert_eq!(
        markers[1].toggle(source).expect("nested marker"),
        source.replacen("[X]", "[ ]", 1)
    );
    assert_eq!(
        markers[2].toggle(source).expect("quoted marker"),
        source.replacen("[x]", "[ ]", 1)
    );
    assert!(markers[1].toggle("short").is_none());
    // The checkbox offset excludes list indentation and other padding.
    for source in ["-   [ ] Row\n", "- [\t] Row\n", ">   -  [X] Row\n"] {
        let markers = tasks(&parse(source).blocks);
        assert_eq!(markers.len(), 1, "{source:?}");
        let at = source.find('[').expect("opening bracket");
        assert_eq!(markers[0].offset as usize, at);
        let toggled = markers[0].toggle(source).expect("marker with padding");
        assert_eq!(&source[..at], &toggled[..at]);
        assert_eq!(&source[at + 3..], &toggled[at + 3..]);
    }
}

proptest! {
    #[test]
    fn arbitrary_notes_never_panic_or_emit_other_schemes(source in any::<String>()) {
        let doc = parse(&source);
        let _ = doc.plain_text();
        for span in spans(&doc.blocks) {
            if let Some(link) = &span.link {
                let destination = url::Url::parse(link.as_str()).expect("absolute URL");
                prop_assert!(["http", "https", "mailto"].contains(&destination.scheme()));
            }
        }
        for marker in tasks(&doc.blocks) {
            let toggled = marker.toggle(&source).expect("parsed marker");
            let at = marker.offset as usize;
            prop_assert_eq!(&toggled[..at], &source[..at]);
            prop_assert_eq!(&toggled[at+3..], &source[at+3..]);
        }
    }

    #[test]
    fn projection_preserves_visible_unicode_and_punctuation(
        words in prop::collection::vec("[a-zA-Z0-9áčž日🙂&.,!?_]{1,24}", 1..16),
        scheme in "[a-z]{1,12}",
    ) {
        let escaped = |word: &str| word.replace('!', "\\!").replace('_', "\\_").replace('&', "&amp;");
        let source = words.iter().map(|word| format!("- **{}** · *{}* · `{word}` · [{}]({scheme}:example.com)", escaped(word), escaped(word), escaped(word)))
            .collect::<Vec<_>>().join("\n");
        let visible = words.iter().map(|word| format!("{word} · {word} · {word} · {word}")).collect::<Vec<_>>().join("\n");
        let doc = parse(&source);
        prop_assert_eq!(doc.plain_text(), visible);
        if scheme != "mailto" {
            prop_assert!(spans(&doc.blocks).iter().all(|s| s.link.is_none()));
        }
    }
}
