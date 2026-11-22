//! The tag picker, with an action page per tag and a rechecked delete confirmation.

use super::{
    Ctx, Detail, backend, ctx, icon, model,
    pickers::{DetailPicker, Swatches},
    platform, room_actions,
};
use leptos::{html, prelude::*};
use todo_client::intent::{self, organize};
use todo_core::{
    State,
    ids::{TagId, TaskId},
    query::fold,
};
use todo_design::Icon;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    All,
    Actions(TagId),
    Colour(TagId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Rename(TagId),
    Delete(TagId),
}

impl Edit {
    fn id(self) -> TagId {
        match self {
            Self::Rename(id) | Self::Delete(id) => id,
        }
    }
}

fn toggle(c: Ctx, task: TaskId, tag: TagId) {
    if c.read_only() {
        return;
    }
    if let Some((command, label)) = c.state.with_untracked(|s| {
        let command = organize::toggle_tag(s, task, tag)?;
        let name = &s.tags.get(&tag)?.name;
        let action = if matches!(command, todo_core::Command::RemoveTag { .. }) {
            "Removed"
        } else {
            "Added"
        };
        Some((command, format!("{action} tag “{name}”")))
    }) {
        c.submit_undoable(command, label, None);
    }
}

#[component]
pub(super) fn TagEditor(task: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let picker = DetailPicker::new(c, task, "edit-task-tags", "tag-picker");
    let page = RwSignal::new(Page::All);
    let filter = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let edit = RwSignal::new(None::<Edit>);
    let dialog = NodeRef::<html::Dialog>::new();
    let go = move |next, target| {
        page.set(next);
        error.set(None);
        picker.focus_page(target);
    };
    let start_edit = move |next: Edit| {
        if c.read_only() {
            return;
        }
        if let Some(dialog) = dialog.get_untracked() {
            picker.close(false);
            edit.set(Some(next));
            if dialog.show_modal().is_err() {
                edit.set(None);
                return;
            }
            request_animation_frame(move || match next {
                Edit::Rename(_) => platform::focus_select_text("tag-name"),
                Edit::Delete(_) => {
                    platform::focus("cancel-tag-edit");
                }
            });
        }
    };
    let create = move || {
        if c.read_only() {
            return;
        }
        let text = filter.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        match c
            .state
            .with_untracked(|s| organize::tag_task_named(s, task, &text, backend::new_uuid))
        {
            Ok((tag, mut commands)) => {
                // The shared builder issues two commands for a new name. Undo the attachment
                // exactly, not the reusable tag another task may already have started using.
                let attach = commands.pop();
                if commands.into_iter().all(|cmd| c.submit(cmd))
                    && attach.is_none_or(|cmd| {
                        c.submit_undoable(cmd, format!("Added tag “{}”", text.trim()), None)
                    })
                {
                    filter.set(String::new());
                    error.set(None);
                    request_animation_frame(move || {
                        picker.position();
                        if picker.open.try_get_untracked() == Some(true) {
                            platform::focus(&format!("pick-tag-{tag}"));
                        }
                    });
                } else {
                    error.set(c.toast.get_untracked().map(|t| t.text));
                }
            }
            Err(e) => error.set(Some(format!("Couldn’t add tag: {e}."))),
        }
    };
    let can_create = Memo::new(move |_| {
        let text = filter.get();
        !text.trim().is_empty() && c.state.with(|s| organize::tag_named(s, &text).is_none())
    });
    let ids = Memo::new(move |_| {
        let words = fold(&filter.get());
        c.state.with(|s| {
            organize::tags(s)
                .into_iter()
                .filter(|tag| {
                    let name = fold(&tag.name);
                    words.split_whitespace().all(|w| name.contains(w))
                })
                .map(|tag| tag.id)
                .collect::<Vec<_>>()
        })
    });
    Effect::new(move |_| {
        if !picker.open.get() {
            return;
        }
        match page.get() {
            Page::All => {
                ids.track();
                filter.track();
                request_animation_frame(move || picker.position());
            }
            Page::Actions(id) | Page::Colour(id) if !c.state.with(|s| s.is_live(id.into())) => {
                go(Page::All, "tag-filter")
            }
            _ => {}
        }
    });
    view! {
        <div class="prop">
            <dt>{icon(Icon::Hash)}"Tags"</dt>
            <dd>
                <button type="button" id=picker.anchor class="prop-edit tag-trigger" aria-label="Tags"
                    aria-haspopup="dialog" aria-controls=picker.id aria-expanded=move || picker.open.get().to_string()
                    popovertarget=picker.id title="Choose tags (T)" aria-keyshortcuts="T" disabled=move || c.read_only()
                    on:click=move |ev| {
                        ev.prevent_default();
                        if picker.open.get_untracked() { picker.close(true); }
                        else { page.set(Page::All); filter.set(String::new()); error.set(None); picker.show("tag-filter"); }
                    }>
                    {move || detail.with(|d| {
                        if let Some(d) = d.as_ref().filter(|d| !d.tags.is_empty()) {
                            d.tags.iter().map(|tag| view! {
                                <span class="detail-tag" data-list-hue=tag.hue><span class="tag-dot"></span>{tag.label.clone()}</span>
                            }).collect_view().into_any()
                        } else { "—".into_any() }
                    })}
                </button>
                <div class="room-action-menu detail-picker tag-picker" role="dialog" aria-labelledby="tag-picker-heading"
                    id=picker.id popover="auto" node_ref=picker.node on:toggle=move |_| picker.toggle_event()
                    on:keydown=move |ev| picker.key(c, Some(if matches!(page.get_untracked(), Page::Colour(_)) { 6 } else { 1 }), &ev)>
                    <Show when=move || picker.open.get()>
                        {move || match page.get() {
                            Page::All => view! {
                                <div class="picker-page">
                                    <h3 id="tag-picker-heading">"Tags"</h3>
                                    <form on:submit=move |ev| { ev.prevent_default(); create(); }>
                                        <input id="tag-filter" type="text" class="room-name-input" autocomplete="off"
                                            aria-label="Type to find or create a tag" placeholder="Type to find or create a tag"
                                            prop:value=move || filter.get() on:input=move |ev| {
                                                filter.set(event_target_value(&ev)); error.set(None);
                                            }
                                            on:keydown=move |ev| {
                                                if !ev.is_composing() && ev.key() == "ArrowDown" {
                                                    ev.prevent_default();
                                                    if !ev.repeat() {
                                                        if can_create.get_untracked() { platform::focus("create-tag"); }
                                                        else if let Some(id) = ids.with_untracked(|v| v.first().copied()) {
                                                            platform::focus(&format!("pick-tag-{id}"));
                                                        }
                                                    }
                                                }
                                            }/>
                                    </form>
                                    <Show when=move || can_create.get()>
                                        <button id="create-tag" type="button" data-picker-option="" on:click=move |_| create()>
                                            {icon(Icon::Plus)}<span class="picker-label">{move || format!("Create tag “{}”", filter.get().trim())}</span>
                                        </button>
                                    </Show>
                                    <Show when=move || ids.with(Vec::is_empty) && filter.with(|s| s.trim().is_empty())>
                                        <p class="picker-note">"No tags yet. Type a name to create one."</p>
                                    </Show>
                                    <For each=move || ids.get() key=|id| *id children=move |id| view! {
                                        <TagChoice task=task id=id manage=Callback::new(move |()| go(Page::Actions(id), "toggle-tag"))/>
                                    }/>
                                    {move || error.get().map(|text| view! { <p role="alert" class="prop-error">{text}</p> })}
                                </div>
                            }.into_any(),
                            Page::Actions(id) => view! {
                                <TagActions task=task id=id edit=Callback::new(start_edit)
                                    colour=Callback::new(move |()| {
                                        go(Page::Colour(id), "tag-swatch-back");
                                        super::pickers::focus_choice(picker.id, "tag-swatch-back");
                                    })
                                    back=Callback::new(move |()| {
                                        go(Page::All, "tag-filter");
                                        request_animation_frame(move || {
                                            if picker.open.try_get_untracked() == Some(true) && page.try_get_untracked() == Some(Page::All) {
                                                platform::focus(&format!("pick-tag-{id}"));
                                            }
                                        });
                                    })/>
                            }.into_any(),
                            Page::Colour(id) => view! {
                                <div class="picker-page">
                                    <h3 id="tag-picker-heading">"Tag colour"</h3>
                                    <Swatches selected=Signal::derive(move || tag_display(c, id).map(|(_, hue)| hue))
                                        disabled=Signal::derive(move || c.read_only())
                                        choose=Callback::new(move |key| {
                                            let cmd = c.state.with_untracked(|s| organize::set_color(s, id.into(), key));
                                            if cmd.is_none_or(|cmd| c.submit_undoable(cmd, "Tag colour changed.".into(), None)) { picker.close(true); }
                                        })/>
                                    <button type="button" id="tag-swatch-back" on:click=move |_| go(Page::Actions(id), "toggle-tag")>
                                        {icon(Icon::ChevronLeft)}"Back"
                                    </button>
                                </div>
                            }.into_any(),
                        }}
                    </Show>
                </div>
                <TagDialog task=task edit=edit node=dialog/>
            </dd>
        </div>
    }
}

fn tag_display(c: Ctx, id: TagId) -> Option<(String, &'static str)> {
    c.state.with(|s| {
        let tag = s.tags.get(&id)?;
        Some((
            tag.name.clone(),
            model::swatch(tag.color.as_deref(), id.as_uuid().as_bytes()),
        ))
    })
}

fn attached(c: Ctx, task: TaskId, id: TagId) -> bool {
    c.state
        .with(|s| s.tasks.get(&task).is_some_and(|t| t.tags.contains(&id)))
}

#[component]
fn TagChoice(task: TaskId, id: TagId, manage: Callback<()>) -> impl IntoView {
    let c = ctx();
    let tag = Memo::new(move |_| tag_display(c, id));
    view! {
        <div class="tag-choice">
            <button type="button" id=format!("pick-tag-{id}") data-picker-option=""
                aria-pressed=move || attached(c, task, id).to_string()
                on:click=move |_| toggle(c, task, id)
                on:keydown=move |ev| {
                    if !ev.is_composing() && !ev.repeat() && !ev.alt_key() && !ev.ctrl_key() && !ev.meta_key() && ev.key() == "ArrowRight" {
                        ev.prevent_default(); manage.run(());
                    }
                }>
                <span class="tag-dot" data-list-hue=move || tag.with(|t| t.as_ref().map(|(_, hue)| *hue))></span>
                <span class="picker-label">{move || tag.with(|t| t.as_ref().map(|(name, _)| name.clone()))}</span>
                <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
            </button>
            <button type="button" class="tag-manage" aria-label=move || tag.with(|t| t.as_ref().map(|(name, _)| format!("Manage tag “{name}”")))
                on:click=move |_| manage.run(())>{icon(Icon::ChevronRight)}</button>
        </div>
    }
}

#[component]
fn TagActions(
    task: TaskId,
    id: TagId,
    edit: Callback<Edit>,
    colour: Callback<()>,
    back: Callback<()>,
) -> impl IntoView {
    let c = ctx();
    view! {
        <div class="picker-page">
            <h3 id="tag-picker-heading">{move || tag_display(c, id).map(|(name, _)| format!("#{name}"))}</h3>
            <button type="button" id="toggle-tag" data-picker-option="" on:click=move |_| toggle(c, task, id)>
                {icon(Icon::Hash)}{move || if attached(c, task, id) { "Remove from task" } else { "Add to task" }}
            </button>
            <button type="button" data-picker-option="" on:click=move |_| edit.run(Edit::Rename(id))>{icon(Icon::Pencil)}"Rename tag"</button>
            <button type="button" data-picker-option="" on:click=move |_| colour.run(())>
                <span class="tag-dot" data-list-hue=move || tag_display(c, id).map(|(_, hue)| hue)></span>"Colour…"
            </button>
            <button type="button" data-picker-option="" class="danger" on:click=move |_| edit.run(Edit::Delete(id))>{icon(Icon::TrashTwo)}"Delete tag…"</button>
            <button type="button" data-picker-option="" on:click=move |_| back.run(())>{icon(Icon::ChevronLeft)}"All tags"</button>
        </div>
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Impact {
    name: String,
    count: usize,
}

impl Impact {
    fn read(state: &State, task: TaskId, tag: TagId) -> Option<Self> {
        let g = state
            .tags
            .get(&tag)
            .filter(|_| state.is_live(tag.into()) && state.is_live(task.into()))?;
        Some(Self {
            name: g.name.clone(),
            count: state
                .tasks
                .values()
                .filter(|t| t.tags.contains(&tag) && state.is_live(t.id.into()))
                .count(),
        })
    }

    fn question(&self) -> String {
        let impact = match self.count {
            0 => String::new(),
            1 => " It’s on 1 task. The task stays.".into(),
            n => format!(" It’s on {n} tasks. The tasks stay."),
        };
        format!(
            "Delete the tag “{}”?{impact} Undo restores the tag.",
            self.name
        )
    }
}

#[component]
fn TagDialog(
    task: TaskId,
    edit: RwSignal<Option<Edit>>,
    node: NodeRef<html::Dialog>,
) -> impl IntoView {
    let close = move || room_actions::close_dialog(node);
    let c = ctx();
    view! {
        <dialog class="room-dialog" node_ref=node aria-labelledby="tag-action-heading"
            aria-describedby="tag-action-description" on:keydown=move |ev| {
                ev.stop_propagation(); room_actions::block_dialog_repeat(&ev);
            }
            on:click=move |ev| { if room_actions::scrim_click(&ev) { close(); } }
            on:close=move |_| {
                edit.set(None);
                if c.selected.get_untracked() == Some(task) { platform::focus("edit-task-tags"); }
            }>
            {move || edit.get().map(|edit| view! { <TagEditForm task=task edit=edit close=Callback::new(move |()| close())/> })}
        </dialog>
    }
}

#[component]
fn TagEditForm(task: TaskId, edit: Edit, close: Callback<()>) -> impl IntoView {
    let c = ctx();
    let id = edit.id();
    let rename = matches!(edit, Edit::Rename(_));
    let asked = RwSignal::new(c.state.with_untracked(|s| Impact::read(s, task, id)));
    let name = asked.with_untracked(|i| i.as_ref().map_or_else(String::new, |i| i.name.clone()));
    let available = move || {
        c.state
            .with(|s| s.is_live(id.into()) && s.is_live(task.into()))
    };
    let error = RwSignal::new(None::<String>);
    let input = NodeRef::<html::Input>::new();
    view! {
        <form on:submit=move |ev| {
            ev.prevent_default();
            if c.read_only() || !available() { return; }
            let saved = if rename {
                let Some(input) = input.get_untracked() else { return };
                match c.state.with_untracked(|s| organize::rename_tag(s, id, &input.value())) {
                    Ok(cmd) => cmd.is_none_or(|cmd| c.submit_undoable(cmd, "Tag renamed.".into(), None)),
                    Err(e) => { error.set(Some(format!("Couldn’t rename tag: {e}."))); return; }
                }
            } else {
                let impact = c.state.with_untracked(|s| Impact::read(s, task, id));
                let Some(impact) = impact else { return };
                if asked.with_untracked(|i| i.as_ref() != Some(&impact)) {
                    asked.set(Some(impact));
                    error.set(Some("This tag changed. Check again before deleting.".into()));
                    platform::focus("cancel-tag-edit");
                    return;
                }
                c.submit_undoable(intent::delete(id), format!("Deleted tag “{}”", impact.name), None)
            };
            if saved { close.run(()); }
            else { error.set(c.toast.get_untracked().map(|t| t.text)); }
        }>
            <h2 id="tag-action-heading">{if rename { "Rename tag" } else { "Delete tag" }}</h2>
            <div id="tag-action-description">
                <Show when=move || !available()><p>"This tag is no longer available."</p></Show>
                {(!rename).then(|| view! {
                    <Show when=available><p>{move || asked.with(|i| i.as_ref().map(Impact::question))}</p></Show>
                })}
            </div>
            {rename.then(|| view! {
                <input type="text" value=name node_ref=input id="tag-name" class="room-name-input" aria-label="Tag name"
                    autocomplete="off" required disabled=move || c.read_only() || !available() on:input=move |_| error.set(None)/>
            })}
            {move || error.get().map(|text| view! { <p role="alert" class="action-error">{text}</p> })}
            <div class="dialog-actions">
                <button type="button" class="button" id="cancel-tag-edit" on:click=move |_| close.run(())>"Cancel"</button>
                <button type="submit" class="button" class:primary=rename class:danger=!rename
                    disabled=move || c.read_only() || !available()>{if rename { "Save" } else { "Delete tag" }}</button>
            </div>
        </form>
    }
}
