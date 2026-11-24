use crate::theme::{Palette, TextStyled, component, radius, space, ty};
use gpui::{
    AnyElement, Div, FontStyle, FontWeight, StyledText, TextRun, div, font, prelude::*, px,
};
use todo_client::assistant::markdown::{MarkdownBlock, MarkdownSpan, parse};

pub(super) fn reply(source: &str, palette: Palette) -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(space::S2))
        .children(
            parse(source)
                .into_iter()
                .map(|block| render(block, palette)),
        )
}

fn render(block: MarkdownBlock, palette: Palette) -> AnyElement {
    match block {
        MarkdownBlock::Heading { level, spans } => inline(
            spans,
            match level {
                1 => ty::NOTES_H1,
                2 => ty::NOTES_H2,
                _ => ty::NOTES_H3,
            },
            palette,
        )
        .into_any_element(),
        MarkdownBlock::Paragraph(spans) => inline(spans, ty::NOTES, palette).into_any_element(),
        MarkdownBlock::UnorderedList(items) => list()
            .children(
                items
                    .into_iter()
                    .map(|spans| row("•".into(), spans, palette)),
            )
            .into_any_element(),
        MarkdownBlock::OrderedList(items) => list()
            .children(
                items
                    .into_iter()
                    .map(|item| row(format!("{}.", item.number), item.spans, palette)),
            )
            .into_any_element(),
        MarkdownBlock::Code(text) => div()
            .min_w_0()
            .p(px(space::S2))
            .bg(palette.wash)
            .rounded(px(radius::BUTTON))
            .style(ty::NOTES_CODE)
            .text_color(palette.ink)
            .child(StyledText::new(text))
            .into_any_element(),
    }
}

fn list() -> Div {
    div().min_w_0().flex().flex_col().gap(px(space::S1))
}

fn row(marker: String, spans: Vec<MarkdownSpan>, palette: Palette) -> Div {
    div()
        .min_w_0()
        .flex()
        .items_start()
        .gap(px(space::S2))
        .child(
            div()
                .flex_none()
                .min_w(px(component::ICON_SMALL))
                .text_right()
                .style(ty::NOTES)
                .text_color(palette.ink_2)
                .child(marker),
        )
        .child(inline(spans, ty::NOTES, palette).flex_1())
}

fn inline(spans: Vec<MarkdownSpan>, style: ty::TextStyle, palette: Palette) -> Div {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    for span in spans {
        let code = matches!(span, MarkdownSpan::Code(_));
        let mut font = font(if code {
            ty::NOTES_CODE.family
        } else {
            style.family
        });
        font.weight = FontWeight(f32::from(style.weight));
        let content = match span {
            MarkdownSpan::Bold(value) => {
                font.weight = FontWeight(f32::from(ty::ASSISTANT_BOLD));
                value
            }
            MarkdownSpan::Italic(value) => {
                font.style = FontStyle::Italic;
                value
            }
            MarkdownSpan::Text(value) | MarkdownSpan::Code(value) => value,
        };
        runs.push(TextRun {
            len: content.len(),
            font,
            color: palette.ink,
            background_color: code.then_some(palette.wash),
            underline: None,
            strikethrough: None,
        });
        text.push_str(&content);
    }
    div()
        .min_w_0()
        .style(style)
        .child(StyledText::new(text).with_runs(runs))
}
