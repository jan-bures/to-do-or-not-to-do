//! Render the shared notes document as DOM nodes, never as HTML strings.

use leptos::{html, prelude::*};
use todo_client::{
    intent,
    notes_markdown::{self, Block, Span, TaskMarker},
};
use todo_core::ids::TaskId;

use super::{Draft, backend, ctx, icon, platform};
use todo_design::Icon;

#[component]
pub(super) fn NotesEditor(id: TaskId, notes: Signal<String>) -> impl IntoView {
    let c = ctx();
    let draft = Draft::new(notes);
    let editing = RwSignal::new(false);
    let input = NodeRef::<html::Textarea>::new();
    let edit = move || {
        if !c.read_only() {
            draft.reset(notes);
            editing.set(true);
        }
    };
    let commit = move || {
        if !editing.get_untracked() {
            return;
        }
        if draft.dirty.get_untracked() && !c.read_only() {
            let text = draft.text.get_untracked();
            if let Some(cmd) = intent::set_notes(&backend::view(), id, &text)
                && !c.submit(cmd)
            {
                return;
            }
        }
        editing.set(false);
        draft.reset(notes);
    };
    Effect::new(move |_| {
        if editing.get()
            && let Some(input) = input.get()
        {
            let _ = input.focus();
        }
    });
    view! {
        {move || {
            if editing.get() {
                return view! {
                    <textarea
                        class="notes"
                        aria-label="Notes"
                        node_ref=input
                        readonly=move || c.read_only()
                        placeholder="Add a note"
                        rows="3"
                        prop:value=move || draft.text.get()
                        on:input=move |ev| draft.input(event_target_value(&ev))
                        on:blur=move |_| commit()
                        on:keydown=move |ev| {
                            if !ev.is_composing() && ev.key() == "Escape" {
                                ev.stop_propagation();
                                if let Some(input) = input.get_untracked() { let _ = input.blur(); }
                            }
                        }
                    ></textarea>
                }.into_any();
            }
            let source = notes.get();
            let doc = notes_markdown::parse(&source);
            let toggle = Callback::new(move |marker: TaskMarker| {
                let state = backend::view();
                if let Some(cmd) = intent::toggle_note_task(&state, id, &source, marker)
                    && c.submit_undoable(cmd, "Updated notes checklist".into(), None)
                {
                    request_animation_frame(move || {
                        platform::focus(&marker_id(marker));
                    });
                }
            });
            let empty = doc.blocks.is_empty();
            view! {
                <div class="notes note-preview" class:editable=move || !c.read_only() on:click=move |_| edit()>
                    {empty.then(|| view! { <p class="note-placeholder">"Add a note"</p> })}
                    {blocks(doc.blocks, toggle)}
                    <button type="button" class="note-edit-target" disabled=move || c.read_only()
                        on:focus=move |_| edit() on:click=move |_| edit()>"Edit notes"</button>
                </div>
            }.into_any()
        }}
    }
}

fn marker_id(marker: TaskMarker) -> String {
    format!("note-task-{}", marker.offset)
}

fn blocks(content: Vec<Block>, toggle: Callback<TaskMarker>) -> Vec<AnyView> {
    content.into_iter().map(|block| match block {
        Block::Paragraph(spans) => view! { <p>{inline(spans)}</p> }.into_any(),
        Block::Heading { level, spans } => match level {
            1 => view! { <h3 class="note-h1">{inline(spans)}</h3> }.into_any(),
            2 => view! { <h4 class="note-h2">{inline(spans)}</h4> }.into_any(),
            _ => view! { <h5 class="note-h3">{inline(spans)}</h5> }.into_any(),
        },
        Block::Quote(children) => view! { <blockquote class="note-quote">{blocks(children, toggle)}</blockquote> }.into_any(),
        Block::Code(text) => view! { <pre class="note-code"><code>{text}</code></pre> }.into_any(),
        Block::Rule => view! { <hr class="note-rule"/> }.into_any(),
        Block::List { start, items } => {
            let c = ctx();
            let rows = items.into_iter().enumerate().map(|(index, item)| {
                let number = start.map_or_else(|| "•".to_owned(), |start| format!("{}.", start.saturating_add(index as u64)));
                let marker = item.task.map_or_else(
                    || view! { <span class="note-marker" aria-hidden="true">{number}</span> }.into_any(),
                    |marker| {
                        let label = item.label();
                        view! {
                            <button type="button" role="checkbox" class="note-task"
                                id=marker_id(marker) aria-checked=marker.checked.to_string() aria-label=label
                                disabled=move || c.read_only() on:click=move |ev| {
                                    ev.stop_propagation();
                                    toggle.run(marker);
                                }>
                                <span class="note-check" aria-hidden="true">{marker.checked.then(|| icon(Icon::Check))}</span>
                            </button>
                        }.into_any()
                    });
                view! { <li role="listitem">{marker}<div class="note-item">{blocks(item.blocks, toggle)}</div></li> }
            }).collect_view();
            if let Some(start) = start {
                view! { <ol class="note-list" role="list" start=start.to_string()>{rows}</ol> }.into_any()
            } else {
                view! { <ul class="note-list" role="list">{rows}</ul> }.into_any()
            }
        }
    }).collect()
}

fn inline(spans: Vec<Span>) -> Vec<AnyView> {
    spans
        .into_iter()
        .map(|span| {
            let mut text = if span.style.code {
                view! { <code class="note-inline-code">{span.text}</code> }.into_any()
            } else {
                span.text.into_any()
            };
            if span.style.bold {
                text = view! { <strong>{text}</strong> }.into_any();
            }
            if span.style.italic {
                text = view! { <em>{text}</em> }.into_any();
            }
            if span.style.strike {
                text = view! { <s>{text}</s> }.into_any();
            }
            if let Some(link) = span.link {
                text = view! {
                    <a href=link.as_str().to_owned() rel="noopener noreferrer" target="_blank"
                        on:click=move |ev| ev.stop_propagation()>{text}</a>
                }
                .into_any();
            }
            text
        })
        .collect()
}
