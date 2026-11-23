use std::collections::BTreeMap;

use leptos::prelude::*;
use todo_client::trash;
use todo_core::ids::EntityId;
use todo_design::{Glyph, Icon};

use super::{RoomId, ctx, glyph, icon, platform};

fn button_id(id: EntityId) -> String {
    format!("restore-{}", id.to_string().replace(' ', "-"))
}

#[component]
pub(super) fn Trash() -> impl IntoView {
    let c = ctx();
    let labels = Memo::new(move |_| {
        c.trash.with(|rows| {
            rows.iter()
                .cloned()
                .map(|row| (row.id, row))
                .collect::<BTreeMap<_, _>>()
        })
    });
    view! {
        <section class="trash-body" aria-label="Deleted items">
            <p class="trash-description">{trash::DESCRIPTION}</p>
            <Show when=move || c.trash.with(Vec::is_empty)>
                <p class="empty-line">{trash::EMPTY}</p>
            </Show>
            <ul class="trash-rows">
                <For each=move || c.trash.with(|rows| rows.iter().map(|row| row.id).collect::<Vec<_>>())
                    key=|id| *id children=move |id| {
                    let row = Memo::new(move |_| labels.with(|m| m.get(&id).cloned()));
                    let lead = match id {
                        EntityId::List(_) => icon(Icon::List).into_any(),
                        EntityId::Collection(_) => glyph(Glyph::Layers).into_any(),
                        _ => icon(Icon::CircleCheck).into_any(),
                    };
                    let restore = move |_| {
                        let Some(row) = row.get_untracked() else { return };
                        let Some(command) = c.state.with_untracked(|s| trash::restore(s, id, row.deleted_at)) else {
                            c.show_toast(trash::CHANGED.into(), false);
                            return;
                        };
                        let next = c.trash.with_untracked(|rows| {
                            let at = rows.iter().position(|r| r.id == id)?;
                            rows[at + 1..].iter().chain(rows[..at].iter().rev())
                                .find(|r| r.blocked.is_none()).map(|r| r.id)
                        });
                        if c.submit_undoable(command, trash::restored(&row.title), None) {
                            request_animation_frame(move || {
                                if c.room.get_untracked() == RoomId::Trash
                                    && !document().active_element().is_some_and(|el| el.id() != button_id(id) && el.tag_name() != "BODY")
                                    && !next.is_some_and(|id| platform::focus(&button_id(id))) {
                                    platform::focus("room-title");
                                }
                            });
                        }
                    };
                    view! {
                        <li class="trash-row">
                            <div class="trash-lead">{lead}</div>
                            <div class="trash-text">
                                <p class="trash-title">{move || row.get().map(|r| r.title)}</p>
                                <p class="trash-meta">{move || row.get().map(|r| r.metadata)}</p>
                                <p class="trash-blocked">{move || row.get().and_then(|r| r.blocked)}</p>
                            </div>
                            <Show when=move || row.with(|r| r.as_ref().is_some_and(|r| r.blocked.is_none()))>
                                <button type="button" class="button trash-restore" id=button_id(id)
                                    aria-label=move || row.get().map(|r| format!("Restore {}", r.title))
                                    disabled=move || c.read_only() on:click=restore>
                                    "Restore"
                                </button>
                            </Show>
                        </li>
                    }
                }/>
            </ul>
        </section>
    }
}
