//! Styled notes in the detail pane. Editing still uses the original TextArea.

use std::{cell::Cell, rc::Rc};

use accesskit::{Action, Role, Toggled};
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, FontStyle, FontWeight, Hsla,
    StrikethroughStyle, StyledText, TextRun, UnderlineStyle, div, font, prelude::*, px,
};
use todo_client::{
    intent,
    notes_markdown::{Block, Document, Span, TaskMarker, parse},
};
use todo_core::ids::TaskId;

use super::{ShellView, undo::EditKind};
use crate::{
    theme::{Palette, TextStyled, component, elevation, radius, space, ty},
    ui::kit,
};

#[derive(Default)]
pub(super) struct Preview {
    source: Rc<str>,
    doc: Document,
}

impl Preview {
    pub(super) fn update(&mut self, source: &str) {
        if source != &*self.source {
            self.source = Rc::from(source);
            self.doc = parse(source);
        }
    }
}

impl ShellView {
    pub(super) fn notes_preview(
        &self,
        id: TaskId,
        source: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let mut fallback = Preview::default();
        let preview = if source == &*self.edit.notes_preview.source {
            &self.edit.notes_preview
        } else {
            // The outgoing pane is paint-only and can still show the previous task's notes.
            fallback.update(source);
            &fallback
        };
        let renderer = Renderer {
            p,
            id,
            preview,
            live,
            link_block: Cell::new(0),
        };
        let mut body = renderer.blocks(&preview.doc.blocks, cx);
        if preview.doc.blocks.is_empty() {
            body = body.child(div().text_color(p.ink_3).child("Add notes"));
        }
        if !live {
            return body.into_any_element();
        }
        let focus = self.edit.notes_focus.clone();
        body.id("notes-preview")
            .cursor(CursorStyle::IBeam)
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.edit.notes_focus, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            // Keep the Tab destination mounted without auto-focusing on mouse-down over links
            // and checkboxes. The raw editor takes over the same focus handle.
            .child(
                div()
                    .id("edit-notes")
                    .absolute()
                    .size_0()
                    .track_focus(&focus)
                    .role(Role::Button)
                    .aria_label("Edit notes")
                    .on_a11y_action(Action::Click, move |_, window, cx| window.focus(&focus, cx)),
            )
            .into_any_element()
    }

    fn toggle_note_task(
        &mut self,
        id: TaskId,
        source: &str,
        task: TaskMarker,
        cx: &mut Context<Self>,
    ) {
        let state = self.store.view();
        if self.repeat_target() != Some(id) || !self.edit.notes_draft.accepts_incoming() {
            return;
        }
        if let Some(cmd) = intent::toggle_note_task(&state, id, source, task) {
            let _ = self.submit_if_kind(cmd, id.into(), EditKind::NotesChecklist, |state| {
                state.is_live(id.into())
                    && state
                        .tasks
                        .get(&id)
                        .is_some_and(|task| task.notes == source)
            });
        }
        self.refresh(cx);
    }
}

struct Renderer<'a> {
    p: Palette,
    id: TaskId,
    preview: &'a Preview,
    live: bool,
    link_block: Cell<usize>,
}

impl Renderer<'_> {
    fn blocks(&self, blocks: &[Block], cx: &mut Context<ShellView>) -> Div {
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .children(blocks.iter().map(|block| self.block(block, cx)))
    }

    fn block(&self, block: &Block, cx: &mut Context<ShellView>) -> AnyElement {
        let p = self.p;
        match block {
            Block::Paragraph(spans) => self.inline(spans, ty::NOTES, p.ink_2),
            Block::Heading { level, spans } => self.inline(
                spans,
                match level {
                    1 => ty::NOTES_H1,
                    2 => ty::NOTES_H2,
                    _ => ty::NOTES_H3,
                },
                p.ink,
            ),
            Block::Quote(blocks) => self
                .blocks(blocks, cx)
                .border_l_1()
                .border_color(p.line_strong)
                .pl(px(space::S2))
                .into_any_element(),
            Block::Code(text) => div()
                .min_w_0()
                .overflow_hidden()
                .p(px(space::S2))
                .bg(p.canvas)
                .rounded(px(radius::BUTTON))
                .style(ty::NOTES_CODE)
                .text_color(p.ink)
                .child(StyledText::new(text.clone()))
                .into_any_element(),
            Block::Rule => div()
                .h(px(elevation::HAIRLINE))
                .bg(p.line_strong)
                .into_any_element(),
            Block::List { start, items } => div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .children(items.iter().enumerate().map(|(index, item)| {
                    let side = if let Some(task) = item.task {
                        self.marker(task, item.label(), cx)
                    } else {
                        let marker = start.map_or_else(
                            || "•".to_owned(),
                            |start| format!("{}.", start.saturating_add(index as u64)),
                        );
                        div()
                            .flex_none()
                            .min_w(px(component::ICON_SMALL))
                            .text_right()
                            .child(marker)
                            .into_any_element()
                    };
                    div()
                        .min_w_0()
                        .flex()
                        .items_start()
                        .gap(px(space::S2))
                        .child(side)
                        .child(self.blocks(&item.blocks, cx).flex_1())
                }))
                .into_any_element(),
        }
    }

    fn marker(&self, task: TaskMarker, label: String, cx: &mut Context<ShellView>) -> AnyElement {
        let p = self.p;
        let marker = div()
            .flex_none()
            .flex()
            .items_center()
            .h(px(ty::NOTES.line_height))
            .child(if task.checked {
                kit::checked(&p, false, component::ICON_SMALL)
            } else {
                kit::checkbox(&p, false, component::ICON_SMALL)
            });
        if !self.live {
            return marker.into_any_element();
        }
        let id = self.id;
        let source = self.preview.source.clone();
        let seen = source.clone();
        let shell = cx.weak_entity();
        marker
            .id(("note-task", task.offset as usize))
            .cursor_pointer()
            .role(Role::CheckBox)
            .aria_label(label)
            .aria_toggled(if task.checked {
                Toggled::True
            } else {
                Toggled::False
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_note_task(id, &source, task, cx);
            }))
            .on_a11y_action(Action::Click, move |_, _, cx| {
                let _ = shell.update(cx, |this, cx| this.toggle_note_task(id, &seen, task, cx));
            })
            .into_any_element()
    }

    fn inline(&self, spans: &[Span], ty: ty::TextStyle, ink: Hsla) -> AnyElement {
        let p = self.p;
        let mut text = String::new();
        let mut runs = Vec::with_capacity(spans.len());
        let mut links = Vec::new();
        for span in spans {
            let start = text.len();
            text.push_str(&span.text);
            let mut font = font(if span.style.code {
                ty::NOTES_CODE.family
            } else {
                ty.family
            });
            let weight = if span.style.bold {
                ty::NOTES_BOLD
            } else {
                ty.weight
            };
            font.weight = FontWeight(f32::from(weight));
            if span.style.italic {
                font.style = FontStyle::Italic;
            }
            if let Some(link) = &span.link {
                links.push((start..text.len(), link.clone()));
            }
            runs.push(TextRun {
                len: span.text.len(),
                font,
                color: if span.link.is_some() {
                    p.accent_strong
                } else if span.style.code {
                    p.ink
                } else {
                    ink
                },
                background_color: span.style.code.then_some(p.canvas),
                underline: span.link.as_ref().map(|_| UnderlineStyle {
                    thickness: px(elevation::HAIRLINE),
                    ..Default::default()
                }),
                strikethrough: span.style.strike.then_some(StrikethroughStyle {
                    thickness: px(elevation::HAIRLINE),
                    color: None,
                }),
            });
        }
        let text = StyledText::new(text).with_runs(runs);
        let layout = text.layout().clone();
        let block = div().min_w_0().style(ty).child(text);
        if self.live && !links.is_empty() {
            let index = self.link_block.get();
            self.link_block.set(index + 1);
            block
                .id(("note-links", index))
                .on_click(move |click: &ClickEvent, _, cx| {
                    if let Ok(index) = layout.index_for_position(click.position())
                        && let Some((_, url)) =
                            links.iter().find(|(range, _)| range.contains(&index))
                    {
                        cx.stop_propagation();
                        cx.open_url(url.as_str());
                    }
                })
                .into_any_element()
        } else {
            block.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, model::hints::Stop, store::Store, theme::Theme};
    use gpui::{TestAppContext, point, size};

    #[gpui::test]
    fn tab_and_click_enter_the_raw_editor_and_a_checklist_change_can_be_undone(
        cx: &mut TestAppContext,
    ) {
        let f = fixtures::by_name("notes-markdown").expect("notes fixture");
        let id = f.ui.task.expect("selected task");
        let store = Rc::new(Store::fixture(&f));
        let original = store.view().tasks[&id].notes.clone();
        cx.update(|cx| cx.bind_keys(crate::ui::text_area::key_bindings()));
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(store.clone(), f.ui, Theme::Light, None, window, cx);
            view.still();
            window.activate_window();
            window.focus(view.stop_handle(Stop::DetailImportant), cx);
            view
        });
        cx.simulate_resize(size(px(1440.), px(900.)));
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(shell.read(cx).edit.notes_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(shell.read(cx).list_focus.is_focused(window)));

        // The first note block starts at the notes-section probe.
        let heading = cx.update(|_, cx| {
            let view = shell.read(cx);
            let bounds = view.probes.borrow()["notes-field"];
            bounds.origin + point(px(space::S2), px(space::S2))
        });
        cx.simulate_click(heading, gpui::Modifiers::default());
        cx.update(|window, cx| assert!(shell.read(cx).edit.notes_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");

        let offset = original.find("[ ]").expect("unchecked fixture item");
        let marker = TaskMarker {
            offset: u32::try_from(offset).expect("marker offset"),
            checked: false,
        };
        cx.update(|window, cx| {
            shell.update(cx, |view, cx| {
                view.toggle_note_task(id, &original, marker, cx);
                assert!(!view.edit.notes_focus.is_focused(window));
                assert_eq!(
                    store.view().tasks[&id].notes,
                    original.replacen("[ ]", "[x]", 1)
                );
                // A stale pointer event cannot overwrite an already changed note.
                view.toggle_note_task(id, &original, marker, cx);
                view.request_undo(cx);
                assert_eq!(store.view().tasks[&id].notes, original);
            });
        });
    }
}
