//! The sidebar: smart lists with Activity and today's agents after Agent activity,
//! collections and lists with their counts, New list and New collection pinned below them, the sync
//! line, and inline renaming (lists, collections, and the steps and tags of the detail pane). Its menus
//! (a list's or collection's context menu, the account menu) are drawn by `menu.rs`. The order is the one
//! every client follows (`docs/design-language.md`, *Sidebar order*).

use gpui::{
    ClickEvent, Context, Div, Entity, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    SharedString, Stateful, Subscription, Window, div, prelude::*, px,
};
use todo_core::ids::{CollectionId, EntityId, ListId, StepId, TagId};

use super::menu::{MenuKind, ring};
use super::{Notice, ShellView, quick_add_colors};
use crate::model::hints::Stop;
use crate::model::layout::{self, Pane};
use crate::model::menus::Page;
use crate::model::{self, RoomKey, Tone, commands, labels};
use crate::theme::{self, Icon, TextStyled, component, desktop, elevation, radius, space, ty};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::kit;

/// How far a sub-row (a collection's list, an agent under Activity) is indented.
const SUB_INDENT: f32 = space::S6 + space::S1;

/// What an inline rename edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InlineTarget {
    List(ListId),
    Collection(CollectionId),
    Step(StepId),
    /// A tag, in the detail pane's Tags row.
    Tag(TagId),
}

impl InlineTarget {
    /// The tag being renamed, if it's a tag. Any tag can be: its editor shows in the detail pane's Tags
    /// row whether or not the open task has it.
    pub(super) fn tag(self) -> Option<TagId> {
        match self {
            Self::Tag(id) => Some(id),
            _ => None,
        }
    }
}

/// An inline rename in progress.
pub(super) struct Inline {
    pub target: InlineTarget,
    pub input: Entity<TextInput>,
    /// The text the editor started with: the name as drawn (`labels::one_line`). Only a change from it is
    /// saved, so opening and leaving the editor never rewrites a stored name.
    seed: String,
    reveal: bool,
    _subscriptions: [Subscription; 2],
}

impl ShellView {
    /// Starts renaming `target` in place, its current name selected, on one line.
    pub(super) fn start_inline(
        &mut self,
        target: InlineTarget,
        text: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_inline(cx);
        let colors = quick_add_colors(&self.palette);
        let seed = labels::one_line(&text);
        let input = cx.new(|cx| {
            let mut i = TextInput::new("", colors, cx);
            i.set_text(seed.clone(), cx);
            i.select_everything(cx);
            i
        });
        let events = cx.subscribe_in(
            &input,
            window,
            |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::Submit => {
                    this.commit_inline(cx);
                    window.focus(&this.list_focus, cx);
                }
                InputEvent::Cancel => {
                    this.inline = None;
                    window.focus(&this.list_focus, cx);
                    cx.notify();
                }
                InputEvent::Changed => {}
            },
        );
        let handle = input.read(cx).focus_handle.clone();
        let blur = cx.on_blur(&handle, window, |this, _, cx| this.commit_inline(cx));
        window.focus(&handle, cx);
        self.inline = Some(Inline {
            target,
            input,
            seed,
            reveal: matches!(target, InlineTarget::List(_) | InlineTarget::Collection(_)),
            _subscriptions: [events, blur],
        });
        cx.notify();
    }

    /// Saves an inline rename, if one is open.
    pub(super) fn commit_inline(&mut self, cx: &mut Context<Self>) {
        let Some(inline) = self.inline.take() else {
            return;
        };
        let text = inline.input.read(cx).content.to_string();
        match inline_command(&self.store.view(), inline.target, &inline.seed, &text) {
            Ok(Some(cmd)) => {
                self.submit(vec![cmd]);
            }
            Ok(None) => {}
            Err(e) => self.notice = Some(Notice::error(e.rename_message())),
        }
        self.refresh(cx);
    }

    /// The inline editor, if `target` is being renamed.
    pub(super) fn inline_for(&self, target: InlineTarget) -> Option<Entity<TextInput>> {
        self.inline
            .as_ref()
            .filter(|i| i.target == target)
            .map(|i| i.input.clone())
    }

    pub(super) fn new_list(
        &mut self,
        collection: Option<CollectionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = ListId::from_uuid(self.store.new_uuid());
        if self.submit(vec![commands::create_list(id, collection, "New list")]) {
            self.select_room(RoomKey::List(id), cx);
            self.start_inline(InlineTarget::List(id), "New list".into(), window, cx);
        }
    }

    /// Adds a collection at the end of the sidebar, opens it and starts naming it in place, as New list
    /// does.
    pub(super) fn new_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = CollectionId::from_uuid(self.store.new_uuid());
        let name = "New collection";
        if self.submit(vec![commands::create_collection(id, name)]) {
            self.select_room(RoomKey::Collection(id), cx);
            self.start_inline(InlineTarget::Collection(id), name.into(), window, cx);
        }
    }

    pub(super) fn nudge(&mut self, id: EntityId, by: i32, cx: &mut Context<Self>) {
        if let Some(cmd) = commands::nudge(&self.store.view(), id, by) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    /// Once the editor has been laid out, scroll its row clear of the viewport edges. Focusing a
    /// pinned button doesn't open an editor, so it never changes the scroll position.
    fn reveal_inline_after_layout(&mut self, window: &Window, cx: &Context<Self>) {
        let Some(inline) = self.inline.as_mut().filter(|i| i.reveal) else {
            return;
        };
        let key = match inline.target {
            InlineTarget::List(id) => {
                // A list added to a folded collection opens the group. Wait for it to finish
                // expanding, so the row isn't clipped and the scroll range includes it.
                if self.shell.sidebar.collections.iter().any(|c| {
                    c.lists.iter().any(|l| l.id == id)
                        && self.motion.sidebar.group(c.id, !c.collapsed, self.frame).0 < 1.
                }) {
                    return;
                }
                RoomKey::List(id)
            }
            InlineTarget::Collection(id) => RoomKey::Collection(id),
            _ => return,
        };
        inline.reveal = false;
        let (target, input) = (inline.target, inline.input.entity_id());
        // Don't reuse geometry from a frame in which the row may have been elsewhere.
        self.nav_bounds.borrow_mut().remove(&key);
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            view.update(cx, |this, cx| {
                if this
                    .inline_for(target)
                    .is_none_or(|i| i.entity_id() != input)
                {
                    return;
                }
                let Some(row) = this.nav_bounds.borrow().get(&key).copied() else {
                    return;
                };
                let scroll = &this.sidebar_scroll;
                let viewport = scroll.bounds();
                let offset = scroll.offset();
                if let Some(y) = layout::reveal_offset(
                    (viewport.top().into(), viewport.bottom().into()),
                    (row.top().into(), row.bottom().into()),
                    offset.y.into(),
                    scroll.max_offset().y.into(),
                    space::S2,
                ) {
                    scroll.set_offset(gpui::point(offset.x, px(y)));
                    cx.notify();
                }
            })
            .ok();
        });
    }

    /// Records where a list's or collection's row was painted (`nav_bounds`).
    fn nav_bounds_probe(&self, key: RoomKey) -> impl IntoElement {
        let bounds = self.nav_bounds.clone();
        gpui::canvas(
            move |b, _, _| {
                bounds.borrow_mut().insert(key, b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// Which row has the ring. A collapsed collection can stand in for the active list.
    fn sidebar_selection(shell: &model::Shell) -> RoomKey {
        shell
            .sidebar
            .collections
            .iter()
            .find(|c| c.selected)
            .map_or(shell.sidebar.selected, |c| RoomKey::Collection(c.id))
    }

    pub(super) fn aim_sidebar(&mut self, next: &model::Shell) {
        let (now, pace) = (self.now(), self.pace());
        for c in &next.sidebar.collections {
            let Some(old) = self
                .shell
                .sidebar
                .collections
                .iter()
                .find(|old| old.id == c.id)
            else {
                continue;
            };
            if old.collapsed == c.collapsed {
                continue;
            }
            self.motion.sidebar.disclose(c.id, !c.collapsed, now, pace);
            if c.collapsed {
                self.closing_groups.insert(c.id, old.lists.clone());
            } else {
                self.closing_groups.remove(&c.id);
            }
        }
        self.motion.sidebar.select(
            Self::sidebar_selection(&self.shell),
            Self::sidebar_selection(next),
            now,
            pace,
        );
    }

    /// The existing inset ring and raised fill transfer to the new active row, not the label geometry.
    fn nav_row(&self, key: RoomKey, on: bool, el: Div) -> Div {
        let p = &self.palette;
        let alpha = self.motion.sidebar.ring(key, on, self.frame);
        el.bg(theme::with_alpha(p.raised, alpha))
            .border(px(elevation::HAIRLINE))
            .border_color(theme::with_alpha(p.line, alpha))
            .when(on, |el| {
                el.text_color(p.ink).font_weight(FontWeight::SEMIBOLD)
            })
    }

    fn clickable(
        &self,
        el: Div,
        id: impl Into<gpui::ElementId>,
        on: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let hover = self.palette.hover;
        el.id(id)
            .when(!on, |el| el.hover(move |s| s.bg(hover)))
            .on_click(cx.listener(f))
    }

    fn list_row(
        &self,
        nav: &model::ListNav,
        indent: f32,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let id = nav.id;
        let on = self.shell.sidebar.selected == RoomKey::List(id);
        let name = match self.inline_for(InlineTarget::List(id)).filter(|_| live) {
            Some(input) => div()
                .flex_1()
                .min_w_0()
                .text_color(p.ink)
                .child(input)
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(nav.list.name.clone())
                .into_any_element(),
        };
        let row = div()
            .relative()
            .h(px(desktop::NAV_SUB_ROW))
            .flex()
            .items_center()
            .gap(px(desktop::NAV_GAP))
            .pl(px(indent))
            .pr(px(desktop::NAV_GAP))
            .rounded(px(radius::MD))
            .style(ty::NAV_SUB)
            .text_color(p.ink_2)
            .child(kit::glyph(
                nav.glyph,
                theme::list_colors(self.theme, nav.list.hue).glyph,
            ))
            .child(name)
            .child(
                div()
                    .style(ty::COUNT)
                    .text_color(p.ink_3)
                    .child(nav.count.to_string()),
            );
        let name = nav.list.name.clone();
        let row = self.nav_row(RoomKey::List(id), on && live, row);
        let element_id = SharedString::from(format!("list-{id}"));
        if !live {
            return row.id(element_id);
        }
        let row = row.child(self.nav_bounds_probe(RoomKey::List(id)));
        self.clickable(row, element_id, on, cx, move |this, e, window, cx| {
            if e.click_count() >= 2 {
                this.start_inline(InlineTarget::List(id), name.clone(), window, cx);
            } else {
                this.select_room(RoomKey::List(id), cx);
            }
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                let kind = MenuKind::List {
                    id,
                    page: Page::Main,
                };
                this.open_menu(kind, Some(e.position), None, window, cx);
            }),
        )
    }

    /// Activity (**Ctrl+H**), with a sub-row for each agent active today. Each opens Activity
    /// filtered to that agent.
    fn activity_rows(&self, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let p = self.palette;
        let t = self.theme;
        let sb = &self.shell.sidebar;
        let on = sb.selected == RoomKey::Activity(None);
        let row = div()
            .h(px(desktop::NAV_ROW))
            .flex()
            .items_center()
            .gap(px(desktop::NAV_GAP))
            .pl(px(space::S2))
            .pr(px(desktop::NAV_GAP))
            .rounded(px(radius::MD))
            .style(ty::NAV)
            .text_color(p.ink_2)
            .child(kit::icon(
                Icon::Clock,
                component::ICON,
                theme::list_colors(t, theme::hues::AGENT_ACTIVITY).glyph,
            ))
            .child(div().flex_1().child("Activity"))
            .child(kit::kbd(&p, "Ctrl H"));
        let activity = self.clickable(
            self.nav_row(RoomKey::Activity(None), on, row),
            "nav-activity",
            on,
            cx,
            |this, _, _, cx| this.select_room(RoomKey::Activity(None), cx),
        );
        let agents = sb.agents.iter().map(|a| {
            let key = RoomKey::Activity(Some(a.key));
            let on = sb.selected == key;
            let row = div()
                .h(px(desktop::NAV_SUB_ROW))
                .flex()
                .items_center()
                .gap(px(desktop::NAV_GAP))
                .pl(px(SUB_INDENT))
                .pr(px(desktop::NAV_GAP))
                .rounded(px(radius::MD))
                .style(ty::NAV_SUB)
                .text_color(p.ink_2)
                .child(kit::agent_mark(t, a.agent, component::AGENT_MARK))
                // Keep short names legible even at the minimum sidebar width. The tooltip
                // has the full name and status, even if a long name also needs to truncate.
                .child(
                    div()
                        .flex_auto()
                        .min_w(px(desktop::NAV_AGENT_NAME_MIN))
                        .truncate()
                        .child(a.name.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .style(ty::COUNT)
                        .text_color(p.ink_3)
                        .child(a.status.clone()),
                );
            self.clickable(
                self.nav_row(key, on, row),
                SharedString::from(format!("agent-{}", a.key.as_str())),
                on,
                cx,
                move |this, _, _, cx| this.select_room(key, cx),
            )
            .tooltip(kit::tooltip(p, format!("{}: {}", a.name, a.status)))
        });
        std::iter::once(activity).chain(agents).collect()
    }

    /// The sync line in the sidebar's footer. While something couldn't sync, it's a button: it opens the
    /// list of refused changes, upwards, to see and dismiss them. Tab reaches it (`Stop::SyncFailures`).
    fn sync_line(&self, dot: gpui::Hsla, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let line = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .style(ty::COUNT)
            .child(div().size(px(component::CHIP_DOT)).rounded_full().bg(dot))
            .child(self.shell.sidebar.sync.clone());
        if self.store.sync().failed == 0 {
            return line;
        }
        let button = line
            .id("sync-failures")
            .relative()
            .h(px(component::ICON_BUTTON))
            .px(px(space::S2))
            .ml(px(-space::S2))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .track_focus(&self.sync_focus)
            .hover(|s| s.bg(p.hover))
            .child(kit::icon(Icon::ChevronUp, component::ICON_SMALL, p.ink_3))
            .on_click(cx.listener(|this, e: &ClickEvent, window, cx| {
                this.toggle_menu(MenuKind::SyncFailures, Stop::SyncFailures, e, window, cx);
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                this.control_key(e, window, cx);
            }))
            .children(self.menu_anchor(|k| matches!(k, MenuKind::SyncFailures), cx));
        ring(&p, self.sync_focus.is_focused(window), radius::MD, button)
    }

    pub(super) fn sidebar(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.reveal_inline_after_layout(window, cx);
        let p = self.palette;
        let t = self.theme;
        let sb = &self.shell.sidebar;
        let count = |n: u32| {
            div()
                .style(ty::COUNT)
                .text_color(p.ink_3)
                .child(n.to_string())
        };

        let mut smart: Vec<_> = sb
            .smart
            .iter()
            .map(|item| {
                let s = item.smart;
                let on = sb.selected == RoomKey::Smart(s);
                let row = div()
                    .h(px(desktop::NAV_ROW))
                    .flex()
                    .items_center()
                    .gap(px(desktop::NAV_GAP))
                    .pl(px(space::S2))
                    .pr(px(desktop::NAV_GAP))
                    .rounded(px(radius::MD))
                    .style(ty::NAV)
                    .text_color(p.ink_2)
                    .child(kit::icon(
                        s.icon(),
                        component::ICON,
                        theme::list_colors(t, s.hue()).glyph,
                    ))
                    .child(div().flex_1().child(s.label()))
                    .children(item.count.map(count))
                    .children(item.pip.clone().map(|pip| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(component::CHIP_GAP))
                            .style(ty::COUNT)
                            .text_color(p.accent_strong)
                            .child(
                                div()
                                    .size(px(component::CHIP_DOT))
                                    .rounded_full()
                                    .bg(p.accent),
                            )
                            .child(pip)
                    }));
                self.clickable(
                    self.nav_row(RoomKey::Smart(s), on, row),
                    SharedString::from(format!("smart-{s:?}")),
                    on,
                    cx,
                    move |this, _, _, cx| this.select_room(RoomKey::Smart(s), cx),
                )
            })
            .collect();
        // Activity right after Agent activity, followed by the agents active today.
        let after = sb.activity_at();
        let activity = self.activity_rows(cx);
        smart.splice(after..after, activity);

        let mut collections = Vec::new();
        for (i, c) in sb.collections.iter().enumerate() {
            let cid = c.id;
            // A gap sets the collections off from the smart lists, and an open collection from the
            // next one. Collapsed collections stack like ordinary rows.
            let gap = i.checked_sub(1).map_or(space::S3, |i| {
                let prev = &sb.collections[i];
                space::S2
                    * self
                        .motion
                        .sidebar
                        .group(prev.id, !prev.collapsed, self.frame)
                        .0
            });
            let (shown, angle) = self.motion.sidebar.group(cid, !c.collapsed, self.frame);
            let on = c.selected;
            let name = match self.inline_for(InlineTarget::Collection(cid)) {
                Some(input) => div().flex_1().min_w_0().child(input).into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(c.name.clone())
                    .into_any_element(),
            };
            let chevron = if angle == 0. {
                kit::icon(Icon::ChevronRight, component::ICON_SMALL, p.ink_3)
            } else {
                kit::icon(Icon::ChevronDown, component::ICON_SMALL, p.ink_3).with_transformation(
                    gpui::Transformation::rotate(gpui::Radians(
                        -(1. - angle) * std::f32::consts::FRAC_PI_2,
                    )),
                )
            };
            let toggle = div()
                .id("toggle-collection")
                .flex_none()
                .h_full()
                .flex()
                .items_center()
                .pl(px(space::S1))
                .cursor_pointer()
                .child(chevron)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_collection(cid, cx);
                }));
            let head = div()
                .relative()
                .h(px(desktop::NAV_SUB_ROW))
                .mt(px(gap))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .pr(px(desktop::NAV_GAP))
                .rounded(px(radius::MD))
                .style(ty::NAV_COLLECTION)
                .text_color(p.ink)
                .child(toggle)
                .child(kit::glyph(c.glyph, theme::list_colors(t, c.hue).glyph))
                .child(name)
                .child(count(c.count))
                .child(self.nav_bounds_probe(RoomKey::Collection(cid)));
            let cname = c.name.clone();
            let head = self
                .clickable(
                    self.nav_row(RoomKey::Collection(cid), on, head),
                    SharedString::from(format!("collection-{cid}")),
                    on,
                    cx,
                    move |this, e, window, cx| {
                        if e.click_count() >= 2 {
                            this.start_inline(
                                InlineTarget::Collection(cid),
                                cname.clone(),
                                window,
                                cx,
                            );
                        } else {
                            this.select_room(RoomKey::Collection(cid), cx);
                        }
                    },
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        let kind = MenuKind::Collection {
                            id: cid,
                            page: Page::Main,
                        };
                        this.open_menu(kind, Some(e.position), None, window, cx);
                    }),
                );
            let lists = self
                .closing_groups
                .get(&cid)
                .map_or(c.lists.as_slice(), Vec::as_slice);
            let group = (shown > 0. && !lists.is_empty()).then(|| {
                div()
                    .flex_none()
                    .overflow_hidden()
                    .h(px(desktop::NAV_SUB_ROW * lists.len() as f32 * shown))
                    .opacity(shown)
                    .child(
                        div().flex().flex_col().children(
                            lists
                                .iter()
                                .map(|l| self.list_row(l, SUB_INDENT, !c.collapsed, cx)),
                        ),
                    )
            });
            collections.push(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(head)
                    .children(group),
            );
        }
        let top: Vec<_> = sb
            .lists
            .iter()
            .map(|l| self.list_row(l, space::S2, true, cx))
            .collect();
        // New list and New collection: quiet rows, each a Tab stop (Enter or Space adds).
        type Add = fn(&mut ShellView, &mut Window, &mut Context<ShellView>);
        let add_row = |id: &'static str, icon: Icon, label: &'static str, stop: Stop, add: Add| {
            let focused = self.stop_handle(stop).is_focused(window);
            let row = div()
                .id(id)
                .relative()
                .h(px(desktop::NAV_SUB_ROW))
                .flex()
                .items_center()
                .gap(px(desktop::NAV_GAP))
                .pl(px(space::S2))
                .style(ty::NAV_SUB)
                .text_color(if focused { p.ink } else { p.ink_3 })
                .cursor_pointer()
                .track_focus(self.stop_handle(stop))
                .hover(|s| s.bg(p.hover))
                .child(kit::icon(icon, component::ICON_SMALL, p.ink_3))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| add(this, window, cx)))
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                    this.control_key(e, window, cx);
                }));
            ring(&p, focused, radius::MD, row)
        };
        let new_list = add_row(
            "new-list",
            Icon::Plus,
            "New list",
            Stop::NewList,
            |this, window, cx| this.new_list(None, window, cx),
        );
        let new_collection = add_row(
            "new-collection",
            Icon::FolderPlus,
            "New collection",
            Stop::NewCollection,
            |this, window, cx| this.new_collection(window, cx),
        );
        // Pinned between the scrolled rooms and the footer, so they never scroll out of view. The
        // insets match the scrolled column's, so the rows line up with the lists above them.
        let adds = div()
            .flex_none()
            .flex()
            .flex_col()
            .py(px(space::S2))
            .pl(px(space::S2))
            .pr(px(space::S3))
            .border_t_1()
            .border_color(p.line)
            .child(new_list)
            .child(new_collection);

        let dot = match sb.tone {
            Tone::Ok => p.success,
            Tone::Busy => p.accent,
            Tone::Problem => p.danger,
        };
        // The account menu opens upwards from the gear. Tab reaches the gear (`hints::tab_stop`).
        let account = div()
            .id("account")
            .relative()
            .size(px(component::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.account_focus)
            .hover(|s| s.bg(p.hover))
            .child(kit::icon(Icon::Settings, component::GLYPH, p.ink_3))
            .on_click(cx.listener(|this, e: &ClickEvent, window, cx| {
                let kind = MenuKind::Account { confirm: false };
                this.toggle_menu(kind, Stop::Account, e, window, cx);
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                this.control_key(e, window, cx);
            }))
            .children(self.menu_anchor(|k| matches!(k, MenuKind::Account { .. }), cx));
        let account = ring(
            &p,
            self.account_focus.is_focused(window),
            radius::MD,
            account,
        );

        div()
            .w(px(self.columns(window).width(Pane::Sidebar)))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .border_r_1()
            .border_color(p.line)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("sidebar-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.sidebar_scroll)
                            .flex()
                            .flex_col()
                            .child(
                                // Rows would shrink as direct children of the height-limited pane.
                                div()
                                    .flex_none()
                                    .min_h_full()
                                    .flex()
                                    .flex_col()
                                    .pt(px(space::S3))
                                    .pl(px(space::S2))
                                    .pr(px(space::S3)) // reserve the scrollbar gutter
                                    .children(smart)
                                    .children(collections)
                                    .when(!top.is_empty(), |d| {
                                        d.child(div().h(px(space::S3))).children(top)
                                    })
                                    .child(div().flex_none().h(px(space::S3))),
                            ),
                    )
                    .child(
                        self.sidebar_bar
                            .overlay(&self.sidebar_scroll, &p, self.animate),
                    ),
            )
            .child(adds)
            .child(
                div()
                    .h(px(desktop::SIDEBAR_FOOTER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(space::S4))
                    .pr(px(space::S2))
                    .border_t_1()
                    .border_color(p.line)
                    .text_color(p.ink_3)
                    .child(self.sync_line(dot, window, cx))
                    .child(account),
            )
    }
}

/// What ending an inline rename saves: nothing unless the text differs from what the editor started with
/// (`seed`), so opening an editor and leaving it, or pressing Enter, leaves the stored name exactly as it
/// was, even one the editor showed differently (a line break drawn as a space).
fn inline_command(
    state: &todo_core::State,
    target: InlineTarget,
    seed: &str,
    text: &str,
) -> Result<Option<todo_core::Command>, commands::TagError> {
    if text == seed {
        return Ok(None);
    }
    Ok(match target {
        InlineTarget::List(id) => commands::rename_list(state, id, text),
        InlineTarget::Collection(id) => commands::rename_collection(state, id, text),
        InlineTarget::Step(id) => commands::retitle_step(state, id, text),
        // A tag's name must stay unique: a taken one is refused with a reason.
        InlineTarget::Tag(id) => return commands::rename_tag(state, id, text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use todo_core::Command;

    #[test]
    fn an_unchanged_inline_rename_saves_nothing() {
        let mut f = fixtures::my_day();
        let (&list, _) = f
            .state
            .lists
            .iter()
            .find(|(_, l)| l.name == "Planning")
            .expect("Planning");
        f.state.lists.get_mut(&list).expect("list").name = "Plan\nning".into();
        let collection = f.state.lists[&list].collection.expect("collection");
        f.state
            .collections
            .get_mut(&collection)
            .expect("collection")
            .name = "Wo\trk".into();
        let step = *f.state.steps.keys().next().expect("a step");
        f.state.steps.get_mut(&step).expect("step").title = "Draft\nthemes".into();
        let targets = [
            (InlineTarget::List(list), "Plan\nning"),
            (InlineTarget::Collection(collection), "Wo\trk"),
            (InlineTarget::Step(step), "Draft\nthemes"),
        ];
        for (target, stored) in targets {
            // The editor starts with the name as drawn, and leaving it untouched saves nothing.
            let seed = labels::one_line(stored);
            assert_ne!(seed, stored);
            assert_eq!(inline_command(&f.state, target, &seed, &seed), Ok(None));
            // A real edit is saved.
            let cmd = inline_command(&f.state, target, &seed, "Renamed").expect("a valid name");
            assert!(
                matches!(
                    cmd,
                    Some(
                        Command::UpdateList { .. }
                            | Command::UpdateCollection { .. }
                            | Command::UpdateStep { .. }
                    )
                ),
                "{target:?}: {cmd:?}"
            );
        }
        // Nothing was written: the stored names are as they were.
        assert_eq!(f.state.lists[&list].name, "Plan\nning");
    }

    #[test]
    fn any_tag_can_be_renamed_not_only_the_open_tasks() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("task");
        // "docs" isn't on the open task, and its rename still opens (in the Tags row).
        let docs = commands::tag_named(&f.state, "docs").expect("docs");
        assert!(!f.state.tasks[&task].tags.contains(&docs));
        assert_eq!(InlineTarget::Tag(docs).tag(), Some(docs));
        let actions: Vec<_> =
            crate::model::menus::items(&crate::model::menus::tag_edit_menu(&f.state, task, docs))
                .map(|i| i.action)
                .collect();
        assert!(actions.contains(&crate::model::menus::MenuAction::RenameTag(docs)));
        let list = *f.state.lists.keys().next().expect("list");
        assert_eq!(InlineTarget::List(list).tag(), None);
    }

    #[test]
    fn renaming_a_tag_in_place_keeps_names_unique() {
        let f = fixtures::my_day();
        let docs = commands::tag_named(&f.state, "docs").expect("docs");
        let target = InlineTarget::Tag(docs);
        assert_eq!(inline_command(&f.state, target, "docs", "docs"), Ok(None));
        assert!(matches!(
            inline_command(&f.state, target, "docs", "documents"),
            Ok(Some(Command::UpdateTag { name: Some(n), .. })) if n == "documents"
        ));
        // Another tag's name, in any case, is refused with a reason.
        assert_eq!(
            inline_command(&f.state, target, "docs", "Legal"),
            Err(commands::TagError::Taken("legal".into()))
        );
        assert_eq!(
            inline_command(&f.state, target, "docs", "  "),
            Err(commands::TagError::Empty)
        );
    }
}
