//! The detail's source cards and web link field.

use super::{Detail, Source, backend, ctx, icon, platform};
use leptos::{html, prelude::*};
use todo_client::intent::organize::{self, LinkError};
use todo_core::ids::TaskId;
use todo_design::Icon;

#[component]
pub(super) fn Links(task: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let input = NodeRef::<html::Input>::new();
    let error = RwSignal::new(None::<String>);
    let sources = Memo::new(move |_| {
        detail.with(|d| d.as_ref().map(|d| d.sources.clone()).unwrap_or_default())
    });
    view! {
        <section class="d-section" aria-label="Links">
            <h3 class="caps d-caps">{move || if sources.with(|links| links.iter().any(|s| matches!(s, Source::Email { .. }))) { "Source" } else { "Links" }}</h3>
            <div class="sources">{move || sources.get().into_iter().map(|source| {
                let removal = match &source {
                    Source::Web { id, title, .. } => Some((*id, title.clone())),
                    Source::Email { .. } => None,
                };
                view! {
                    <div class="source-row">
                        {source_card(source)}
                        {removal.map(|(id, title)| {
                            let label = format!("Remove link “{title}”");
                            view! {
                                <button type="button" class="icon-button" aria-label=label.clone() title=label
                                    disabled=move || c.read_only() on:click=move |_| {
                                        if let Some(cmd) = c.state.with_untracked(|s| organize::delete_link(s, id))
                                            && c.submit_undoable(cmd, "Link removed.".into(), None)
                                        {
                                            platform::focus("add-task-link");
                                        }
                                    }>{icon(Icon::TrashTwo)}</button>
                            }
                        })}
                    </div>
                }
            }).collect_view()}</div>
            <form class="add-link" on:submit=move |ev| {
                ev.prevent_default();
                if c.read_only() { return; }
                let Some(input) = input.get_untracked() else { return };
                let text = input.value();
                match c.state.with_untracked(|s| organize::add_link(s, task, &text, backend::new_uuid)) {
                    Ok(cmd) => {
                        if c.submit_undoable(cmd, "Link added.".into(), None) {
                            input.set_value("");
                            error.set(None);
                            let _ = input.focus();
                        } else { error.set(c.toast.get_untracked().map(|t| t.text)); }
                    }
                    Err(LinkError::Empty) => {},
                    Err(LinkError::TaskGone) => error.set(Some("That task is gone.".into())),
                    Err(LinkError::NotWeb) => error.set(Some(format!(
                        "“{}” isn't a web address. Try https://example.com or example.com.", text.trim()
                    ))),
                }
            }>
                {icon(Icon::Link)}
                <input id="add-task-link" type="text" inputmode="url" autocomplete="off" node_ref=input aria-label="Add a web link"
                    placeholder="Add a web link…" aria-keyshortcuts="L" aria-describedby=move || error.with(|e| e.as_ref().map(|_| "link-address-error"))
                    disabled=move || c.read_only() on:input=move |_| error.set(None)
                    on:keydown=move |ev| {
                        if ev.key() == "Escape" && !ev.is_composing() && input.get_untracked().is_some_and(|i| !i.value().is_empty()) {
                            ev.stop_propagation();
                            if !ev.repeat() {
                                if let Some(i) = input.get_untracked() { i.set_value(""); }
                                error.set(None);
                            }
                        }
                    }/>
                <button class="icon-button" type="submit" aria-label="Add link" title="Add link"
                    disabled=move || c.read_only()>{icon(Icon::Plus)}</button>
            </form>
            {move || error.get().map(|message| view! { <p class="prop-error" id="link-address-error" role="alert">{message}</p> })}
        </section>
    }
}

fn source_card(s: Source) -> impl IntoView {
    let (lead, title, meta, href) = match s {
        Source::Email {
            subject,
            meta,
            href,
        } => (Icon::Mail, subject, meta, href),
        Source::Web { title, href, .. } => (Icon::Link, title, href.clone(), Some(href)),
    };
    let inner = move |arrow: bool| {
        view! {
            <span class="source-icon">{icon(lead)}</span>
            <span class="source-text">
                <span class="source-title">{title.clone()}</span>
                <span class="source-meta">{meta.clone()}</span>
            </span>
            {arrow.then(|| icon(Icon::ArrowUpRight))}
        }
    };
    match href {
        Some(h) => view! {
            <a class="source" href=h target="_blank" rel="noopener noreferrer">{inner(true)}</a>
        }
        .into_any(),
        None => view! { <div class="source">{inner(false)}</div> }.into_any(),
    }
}
