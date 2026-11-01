use std::cell::Cell;
use std::rc::Rc;

use api::graph::CardView;
use api::ids::NodeId;
use api::settings::ArrangeDirection;
use gpui::prelude::FluentBuilder;
use gpui::{
    Bounds, Context, Div, FontWeight, InteractiveElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Stateful, StatefulInteractiveElement, Styled, Window,
    canvas, deferred, div, px, relative,
};

use super::Dock;
use super::ui::{self, TEXT};
use crate::strings::Str;
use crate::theme::Colors;
use crate::views::icons::{Lucide, icon};
use crate::views::tooltip::tip;

const DIRECTIONS: [ArrangeDirection; 4] = [
    ArrangeDirection::Down,
    ArrangeDirection::Right,
    ArrangeDirection::Up,
    ArrangeDirection::Left,
];
const ITEMS: usize = 5;
const MENU_WIDTH: f32 = 180.0;
const MENU_GAP: f32 = 4.0;
const HEIGHT: f32 = 30.0;

fn glyph(direction: ArrangeDirection) -> Lucide {
    match direction {
        ArrangeDirection::Down => Lucide::ArrowDown,
        ArrangeDirection::Right => Lucide::ArrowRight,
        ArrangeDirection::Up => Lucide::ArrowUp,
        ArrangeDirection::Left => Lucide::ArrowLeft,
    }
}

fn name(direction: ArrangeDirection) -> Str {
    match direction {
        ArrangeDirection::Down => Str::GraphDirectionsDown,
        ArrangeDirection::Right => Str::GraphDirectionsRight,
        ArrangeDirection::Up => Str::GraphDirectionsUp,
        ArrangeDirection::Left => Str::GraphDirectionsLeft,
    }
}

fn part(id: &'static str, c: &Colors) -> gpui::Stateful<Div> {
    let hover = c.surface_hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(HEIGHT))
        .px(px(12.0))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.surface)
        .text_size(px(TEXT))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.ink)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
}

impl Dock {
    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.arrange_menu.take().is_some() {
            cx.notify();
        }
    }

    fn resettable(&self, cx: &Context<Self>) -> Vec<NodeId> {
        let store = self.store.read(cx);
        let Some(snapshot) = store.snapshot() else {
            return Vec::new();
        };
        let default = store.settings().card_view;
        snapshot
            .sizes
            .iter()
            .filter(|size| {
                snapshot.nodes.iter().any(|node| {
                    node.id == size.node && super::card_view_of(node, default) == CardView::Standard
                })
            })
            .map(|size| size.node)
            .collect()
    }

    fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let direction = self.store.read(cx).settings().arrange_direction;
        let resettable = !self.resettable(cx).is_empty();
        self.arrange_menu = Some(resettable);
        let at = DIRECTIONS.iter().position(|d| *d == direction).unwrap_or(0);
        window.focus(&self.menu_focus[at], cx);
        cx.notify();
    }

    fn choose(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let resettable = self.arrange_menu == Some(true);
        self.arrange_menu = None;
        if let Some(&direction) = DIRECTIONS.get(at) {
            self.store
                .update(cx, |store, cx| store.arrange(direction, cx));
        } else if resettable {
            let nodes = self.resettable(cx);
            let task = self
                .store
                .update(cx, |store, cx| store.clear_sizes(nodes, cx));
            let store = self.store.clone();
            cx.spawn(async move |_, cx| {
                if task.await.is_ok() {
                    store.update(cx, |store, cx| store.reload(cx));
                }
            })
            .detach();
        }
        window.focus(&self.caret_focus, cx);
        cx.notify();
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let resettable = self.arrange_menu == Some(true);
        let live: Vec<usize> = (0..ITEMS).filter(|&at| at < 4 || resettable).collect();
        let now = live
            .iter()
            .position(|&at| self.menu_focus[at].is_focused(window));
        let step: isize = match event.keystroke.key.as_str() {
            "down" => 1,
            "up" => -1,
            "escape" => {
                self.arrange_menu = None;
                window.focus(&self.caret_focus, cx);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            "enter" | "space" => {
                if let Some(now) = now {
                    self.choose(live[now], window, cx);
                    cx.stop_propagation();
                }
                return;
            }
            _ => return,
        };
        let count = live.len() as isize;
        let next = now.map_or(0, |now| (now as isize + step).rem_euclid(count) as usize);
        window.focus(&self.menu_focus[live[next]], cx);
        cx.stop_propagation();
    }

    pub(super) fn arrange_bar(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let settings = self.store.read(cx).settings();
        let (direction, always) = (settings.arrange_direction, settings.always_arrange);
        let store = self.store.clone();
        let run = part("arrange-run", &c)
            .flex_1()
            .rounded_l(px(6.0))
            .child(icon(Lucide::Workflow, 14.0, c.ink))
            .child(ui::say(cx, Str::GraphArrange))
            .on_click({
                let store = store.clone();
                move |_, _, cx| store.update(cx, |store, cx| store.arrange(direction, cx))
            });
        let keep = part("arrange-keep", &c)
            .border_l_0()
            .when(always, |keep| {
                keep.bg(c.accent)
                    .border_color(c.accent)
                    .text_color(c.accent_ink)
            })
            .tooltip(tip(ui::say(cx, Str::GraphKeepArrangedHint).to_owned()))
            .child(ui::say(cx, Str::GraphKeepArranged))
            .on_click(move |_, _, cx| {
                store.update(cx, |store, cx| store.keep_arranged(!always, cx))
            });
        let anchor = self.caret.clone();
        let open = self.arrange_menu.is_some();
        let caret = part("arrange-direction", &c)
            .track_focus(&self.caret_focus)
            .border_l_0()
            .rounded_r(px(6.0))
            .justify_center()
            .gap_0()
            .px(px(6.0))
            .tooltip(tip(ui::say(cx, Str::GraphDirection).to_owned()))
            .child(icon(glyph(direction), 14.0, c.ink))
            .child(icon(Lucide::ChevronDown, 12.0, c.ink))
            .child(
                canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| ())
                    .absolute()
                    .size_full(),
            )
            .relative()
            .on_click(cx.listener(move |dock, _, window, cx| {
                if open {
                    dock.close_menu(cx);
                } else {
                    dock.open_menu(window, cx);
                }
            }));
        let menu = self
            .arrange_menu
            .map(|resettable| deferred(self.menu(direction, resettable, &c, cx)).with_priority(1));
        div()
            .relative()
            .flex()
            .child(run)
            .child(keep)
            .child(caret)
            .children(menu)
    }

    fn menu(
        &self,
        direction: ArrangeDirection,
        resettable: bool,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let caret = self.caret.clone();
        let item = |at: usize, label: &str, glyph_name: Lucide, on: bool, enabled: bool| {
            let hover = c.surface_hover;
            div()
                .id(("arrange-item", at))
                .track_focus(&self.menu_focus[at])
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(HEIGHT))
                .px(px(8.0))
                .rounded(px(6.0))
                .text_size(px(TEXT))
                .text_color(if enabled { c.ink } else { c.ink_3 })
                .when(enabled, |item| {
                    item.cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .focus(move |style| style.bg(hover))
                        .on_click(
                            cx.listener(move |dock, _, window, cx| dock.choose(at, window, cx)),
                        )
                })
                .child(icon(
                    glyph_name,
                    14.0,
                    if enabled { c.ink } else { c.ink_3 },
                ))
                .child(label.to_owned())
                .when(on, |item| {
                    item.child(div().flex_1())
                        .child(icon(Lucide::Check, 14.0, c.ink_2))
                })
        };
        let rows = DIRECTIONS.iter().enumerate().map(|(at, &option)| {
            item(
                at,
                ui::say(cx, name(option)),
                glyph(option),
                option == direction,
                true,
            )
        });
        div()
            .id("arrange-menu")
            .absolute()
            .top(relative(1.0))
            .mt(px(MENU_GAP))
            .right_0()
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .p(px(6.0))
            .rounded(px(10.0))
            .bg(c.surface)
            .shadow(vec![gpui::BoxShadow {
                color: c.shadow.into(),
                offset: gpui::point(px(0.0), px(12.0)),
                blur_radius: px(32.0),
                spread_radius: px(0.0),
                inset: false,
            }])
            .occlude()
            .on_key_down(cx.listener(Self::menu_key))
            .on_mouse_down_out(cx.listener(move |dock, e: &MouseDownEvent, _, cx| {
                if e.button == MouseButton::Left && !contains(&caret.get(), e) {
                    dock.close_menu(cx);
                }
            }))
            .children(rows)
            .child(div().h(px(1.0)).mx(px(2.0)).my(px(4.0)).bg(c.line))
            .child(item(
                4,
                ui::say(cx, Str::GraphResetSizes),
                Lucide::RotateCcw,
                false,
                resettable,
            ))
    }
}

fn contains(bounds: &Bounds<Pixels>, event: &MouseDownEvent) -> bool {
    bounds.contains(&event.position)
}

pub(super) fn new_caret() -> Rc<Cell<Bounds<Pixels>>> {
    Rc::new(Cell::new(Bounds::default()))
}
