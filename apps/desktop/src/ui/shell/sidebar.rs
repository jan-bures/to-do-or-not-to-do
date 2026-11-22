//! The sidebar: smart lists, collections and lists with their counts, the agents seen today and the sync
//! line, plus the popover menus (a list's or collection's context menu, the account menu) and inline
//! renaming.

use gpui::{
    ClickEvent, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Pixels, Point,
    SharedString, Stateful, Subscription, Window, anchored, deferred, div, prelude::*, px,
};
use todo_core::ids::{CollectionId, EntityId, ListId, StepId};

use super::{ShellEvent, ShellView, quick_add_colors};
use crate::model::{self, RoomKey, Tone, commands, labels};
use crate::theme::{self, Icon, TextStyled, component, desktop, elevation, radius, space, ty};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::kit;

/// An open popover menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Menu {
    pub at: Point<Pixels>,
    pub kind: MenuKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MenuKind {
    /// The sidebar footer's menu: the server and Sign out (`confirm` once asked).
    Account {
        confirm: bool,
    },
    /// A list's context menu (`confirm` once Delete was chosen).
    List {
        id: ListId,
        confirm: bool,
    },
    Collection {
        id: CollectionId,
    },
}

/// What an inline rename edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InlineTarget {
    List(ListId),
    Collection(CollectionId),
    Step(StepId),
}

/// An inline rename in progress.
pub(super) struct Inline {
    pub target: InlineTarget,
    pub input: Entity<TextInput>,
    /// The text the editor started with: the name as drawn (`labels::one_line`). Only a change from it is
    /// saved, so opening and leaving the editor never rewrites a stored name.
    seed: String,
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
        if let Some(cmd) = inline_command(&self.store.view(), inline.target, &inline.seed, &text) {
            self.submit(vec![cmd]);
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

    fn new_list(
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

    fn nudge(&mut self, id: EntityId, by: i32, cx: &mut Context<Self>) {
        if let Some(cmd) = commands::nudge(&self.store.view(), id, by) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    fn open_menu(&mut self, at: Point<Pixels>, kind: MenuKind, cx: &mut Context<Self>) {
        self.menu = Some(Menu { at, kind });
        cx.notify();
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
            .h(px(desktop::NAV_SUB_ROW))
            .flex()
            .items_center()
            .gap(px(space::S2 + 2.))
            .pl(px(indent))
            .pr(px(space::S2 + 2.))
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
        self.clickable(row, element_id, on, cx, move |this, e, window, cx| {
            if e.click_count() >= 2 {
                this.start_inline(InlineTarget::List(id), name.clone(), window, cx);
            } else {
                this.select_room(RoomKey::List(id), cx);
            }
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                this.open_menu(e.position, MenuKind::List { id, confirm: false }, cx);
            }),
        )
    }

    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let t = self.theme;
        let sb = &self.shell.sidebar;
        let count = |n: u32| {
            div()
                .style(ty::COUNT)
                .text_color(p.ink_3)
                .child(n.to_string())
        };

        let smart: Vec<_> = sb
            .smart
            .iter()
            .map(|item| {
                let s = item.smart;
                let on = sb.selected == RoomKey::Smart(s);
                let row = div()
                    .h(px(desktop::NAV_ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S2 + 2.))
                    .pl(px(space::S2))
                    .pr(px(space::S2 + 2.))
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
                .h(px(desktop::NAV_SUB_ROW))
                .mt(px(gap))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .pr(px(space::S2 + 2.))
                .rounded(px(radius::MD))
                .style(ty::NAV_COLLECTION)
                .text_color(p.ink)
                .child(toggle)
                .child(kit::glyph(c.glyph, theme::list_colors(t, c.hue).glyph))
                .child(name)
                .child(count(c.count));
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
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        this.open_menu(e.position, MenuKind::Collection { id: cid }, cx);
                    }),
                );
            let lists = self
                .closing_groups
                .get(&cid)
                .map_or(c.lists.as_slice(), Vec::as_slice);
            let group =
                (shown > 0. && !lists.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .overflow_hidden()
                        .h(px(desktop::NAV_SUB_ROW * lists.len() as f32 * shown))
                        .opacity(shown)
                        .child(
                            div().flex().flex_col().children(lists.iter().map(|l| {
                                self.list_row(l, space::S6 + space::S1, !c.collapsed, cx)
                            })),
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
        let new_list = div()
            .id("new-list")
            .mt(px(space::S2))
            .h(px(desktop::NAV_SUB_ROW))
            .flex()
            .items_center()
            .gap(px(space::S2 + 2.))
            .pl(px(space::S2))
            .rounded(px(radius::MD))
            .style(ty::NAV_SUB)
            .text_color(p.ink_3)
            .hover(|s| s.bg(p.hover))
            .child(kit::icon(Icon::Plus, component::ICON_SMALL, p.ink_3))
            .child("New list")
            .on_click(cx.listener(|this, _, window, cx| this.new_list(None, window, cx)));

        let agents = (!sb.agents.is_empty()).then(|| {
            div()
                .border_t_1()
                .border_color(p.line)
                .pt(px(space::S3))
                .px(px(space::S1))
                .pb(px(space::S2))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .px(px(space::S1))
                        .mb(px(space::S1))
                        .child(kit::label(&p, "Agents"))
                        .child(kit::label(&p, "Today")),
                )
                .children(sb.agents.iter().map(|a| {
                    div()
                        .h(px(desktop::NAV_ROW))
                        .flex()
                        .items_center()
                        .gap(px(space::S2 + 2.))
                        .px(px(space::S1))
                        .style(ty::NAV_SUB.with_weight(500))
                        .text_color(p.ink_2)
                        .child(kit::agent_mark(t, a.agent, component::AGENT_MARK))
                        .child(div().flex_1().child(a.name.clone()))
                        .child(
                            div()
                                .style(ty::COUNT)
                                .text_color(p.ink_3)
                                .child(a.status.clone()),
                        )
                }))
        });

        let dot = match sb.tone {
            Tone::Ok => p.success,
            Tone::Busy => p.accent,
            Tone::Problem => p.danger,
        };
        let account = div()
            .id("account")
            .size(px(component::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::MD))
            .hover(|s| s.bg(p.hover))
            .child(kit::icon(Icon::Settings, component::GLYPH, p.ink_3))
            .on_click(cx.listener(|this, e: &ClickEvent, _, cx| {
                this.open_menu(e.position(), MenuKind::Account { confirm: false }, cx);
            }));

        div()
            .w(px(desktop::SIDEBAR))
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
                                    .child(new_list)
                                    .child(div().flex_1().min_h(px(space::S3)))
                                    .children(agents),
                            ),
                    )
                    .child(
                        self.sidebar_bar
                            .overlay(&self.sidebar_scroll, &p, self.animate),
                    ),
            )
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .style(ty::COUNT)
                            .child(div().size(px(component::CHIP_DOT)).rounded_full().bg(dot))
                            .child(sb.sync.clone()),
                    )
                    .child(account),
            )
    }

    pub(super) fn menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let menu = self.menu.clone()?;
        let p = self.palette;
        let at = menu.at;
        let item = |id: &'static str, icon: Icon, label: SharedString, danger: bool| {
            let ink = if danger { p.danger_ink } else { p.ink };
            div()
                .id(id)
                .mx(px(space::S1))
                .h(px(desktop::NAV_ROW))
                .px(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S2 + 2.))
                .rounded(px(radius::MD))
                .style(ty::NAV_SUB)
                .text_color(ink)
                .hover(|s| s.bg(p.hover))
                .child(kit::icon(
                    icon,
                    component::ICON_SMALL,
                    if danger { p.danger } else { p.ink_3 },
                ))
                .child(label)
        };
        let close = |this: &mut Self, cx: &mut Context<Self>| {
            this.menu = None;
            cx.notify();
        };
        let state = self.store.view();
        let body = match menu.kind {
            MenuKind::Account { confirm } => {
                let server = self
                    .store
                    .session()
                    .map(|s| SharedString::from(s.server().to_string()))
                    .unwrap_or_else(|| "Sample data (not signed in)".into());
                kit::overlay(&p)
                    .w(px(space::S10 * 6.))
                    .child(
                        div()
                            .px(px(space::S3))
                            .pt(px(space::S2))
                            .pb(px(space::S1))
                            .child(kit::label(&p, "Signed in to")),
                    )
                    .child(
                        div()
                            .px(px(space::S3))
                            .pb(px(space::S2))
                            .truncate()
                            .style(ty::TAG)
                            .text_color(p.ink_2)
                            .child(server),
                    )
                    .child(
                        div()
                            .h(px(elevation::HAIRLINE))
                            .bg(p.line)
                            .mb(px(space::S1)),
                    )
                    .child(if confirm {
                        item(
                            "sign-out-confirm",
                            Icon::LogOut,
                            "Sign out and remove local data".into(),
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.emit(ShellEvent::SignOut);
                        }))
                    } else {
                        item("sign-out", Icon::LogOut, "Sign out…".into(), false).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.open_menu(at, MenuKind::Account { confirm: true }, cx);
                            }),
                        )
                    })
            }
            MenuKind::List { id, confirm } => {
                let list = state.lists.get(&id)?;
                let name: SharedString = labels::one_line(&list.name).into();
                let open = state
                    .tasks
                    .values()
                    .filter(|t| t.list == id && t.deleted.is_none())
                    .count();
                let mut body = kit::overlay(&p).w(px(space::S10 * 6.));
                if confirm {
                    let what = match open {
                        0 => format!("Delete “{name}”"),
                        1 => format!("Delete “{name}” and its task"),
                        n => format!("Delete “{name}” and its {n} tasks"),
                    };
                    body = body.child(
                        item("list-delete-confirm", Icon::TrashTwo, what.into(), true).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.submit(vec![commands::delete(id)]);
                                close(this, cx);
                                this.refresh(cx);
                            }),
                        ),
                    );
                } else {
                    body = body
                        .child(
                            item("list-rename", Icon::Pencil, "Rename".into(), false).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    close(this, cx);
                                    this.start_inline(
                                        InlineTarget::List(id),
                                        name.clone(),
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                        )
                        .child(
                            item("list-up", Icon::ChevronUp, "Move up".into(), false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    close(this, cx);
                                    this.nudge(id.into(), -1, cx);
                                }),
                            ),
                        )
                        .child(
                            item("list-down", Icon::ChevronDown, "Move down".into(), false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    close(this, cx);
                                    this.nudge(id.into(), 1, cx);
                                })),
                        )
                        .child(
                            div()
                                .h(px(elevation::HAIRLINE))
                                .bg(p.line)
                                .my(px(space::S1)),
                        )
                        .child(
                            item("list-delete", Icon::TrashTwo, "Delete list…".into(), true)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_menu(at, MenuKind::List { id, confirm: true }, cx);
                                })),
                        );
                }
                body
            }
            MenuKind::Collection { id } => {
                let c = state.collections.get(&id)?;
                let name: SharedString = labels::one_line(&c.name).into();
                let (icon, label) = if self.ui.collapsed_collections.contains(&id) {
                    (Icon::ChevronDown, "Expand")
                } else {
                    (Icon::ChevronRight, "Collapse")
                };
                kit::overlay(&p)
                    .w(px(space::S10 * 6.))
                    .child(
                        item("collection-toggle", icon, label.into(), false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.toggle_collection(id, cx);
                            },
                        )),
                    )
                    .child(
                        item(
                            "collection-new-list",
                            Icon::ListPlus,
                            "New list here".into(),
                            false,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                close(this, cx);
                                this.new_list(Some(id), window, cx);
                            },
                        )),
                    )
                    .child(
                        item("collection-rename", Icon::Pencil, "Rename".into(), false).on_click(
                            cx.listener(move |this, _, window, cx| {
                                close(this, cx);
                                this.start_inline(
                                    InlineTarget::Collection(id),
                                    name.clone(),
                                    window,
                                    cx,
                                );
                            }),
                        ),
                    )
                    .child(
                        item("collection-up", Icon::ChevronUp, "Move up".into(), false).on_click(
                            cx.listener(move |this, _, _, cx| {
                                close(this, cx);
                                this.nudge(id.into(), -1, cx);
                            }),
                        ),
                    )
                    .child(
                        item(
                            "collection-down",
                            Icon::ChevronDown,
                            "Move down".into(),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            close(this, cx);
                            this.nudge(id.into(), 1, cx);
                        })),
                    )
            }
        };
        Some(deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(px(space::S2))
                .child(body.id("menu").occlude().on_mouse_down_out(cx.listener(
                    |this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    },
                ))),
        ))
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
) -> Option<todo_core::Command> {
    if text == seed {
        return None;
    }
    match target {
        InlineTarget::List(id) => commands::rename_list(state, id, text),
        InlineTarget::Collection(id) => commands::rename_collection(state, id, text),
        InlineTarget::Step(id) => commands::retitle_step(state, id, text),
    }
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
            assert_eq!(inline_command(&f.state, target, &seed, &seed), None);
            // A real edit is saved.
            let cmd = inline_command(&f.state, target, &seed, "Renamed");
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
}
