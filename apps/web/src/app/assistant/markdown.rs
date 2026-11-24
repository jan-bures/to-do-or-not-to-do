use leptos::prelude::*;
use todo_client::assistant::markdown::{MarkdownBlock, MarkdownSpan, parse};

pub(super) fn reply(source: &str) -> impl IntoView + use<> {
    parse(source).into_iter().map(block).collect_view()
}

fn block(block: MarkdownBlock) -> AnyView {
    match block {
        MarkdownBlock::Paragraph(spans) => view! { <p>{inline(spans)}</p> }.into_any(),
        MarkdownBlock::Heading { level, spans } => match level {
            1 => view! { <h4>{inline(spans)}</h4> }.into_any(),
            2 => view! { <h5>{inline(spans)}</h5> }.into_any(),
            _ => view! { <h6>{inline(spans)}</h6> }.into_any(),
        },
        MarkdownBlock::UnorderedList(items) => view! {
            <ul>{items.into_iter().map(|spans| view! { <li>{inline(spans)}</li> }).collect_view()}</ul>
        }.into_any(),
        MarkdownBlock::OrderedList(items) => view! {
            <ol>{items.into_iter().map(|item| view! {
                <li value=item.number.to_string()>{inline(item.spans)}</li>
            }).collect_view()}</ol>
        }.into_any(),
        MarkdownBlock::Code(text) => view! { <pre><code>{text}</code></pre> }.into_any(),
    }
}

fn inline(spans: Vec<MarkdownSpan>) -> impl IntoView {
    spans
        .into_iter()
        .map(|span| match span {
            MarkdownSpan::Text(text) => text.into_any(),
            MarkdownSpan::Bold(text) => view! { <strong>{text}</strong> }.into_any(),
            MarkdownSpan::Italic(text) => view! { <em>{text}</em> }.into_any(),
            MarkdownSpan::Code(text) => view! { <code>{text}</code> }.into_any(),
        })
        .collect_view()
}
