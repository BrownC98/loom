use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, Render, SharedString, Size, StatefulInteractiveElement,
    Styled, Window, anchored, deferred, div, point, px,
};

use crate::float::place::measure;
use crate::views::dock::ui::{TEXT, colors};
use crate::views::icons::{Lucide, icon};

const HEIGHT: f32 = 30.0;
const WIDTH: f32 = 180.0;
const ROW: f32 = 28.0;

pub enum SelectEvent {
    Pick(usize),
}

pub struct Select {
    id: &'static str,
    full: bool,
    size: Rc<Cell<Size<Pixels>>>,
    items: Vec<SharedString>,
    current: usize,
    open: bool,
    focus: FocusHandle,
}

impl EventEmitter<SelectEvent> for Select {}

impl Select {
    pub fn new(id: &'static str, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            full: false,
            size: Rc::default(),
            items: Vec::new(),
            current: 0,
            open: false,
            focus: cx.focus_handle().tab_stop(true),
        }
    }

    pub fn full(mut self) -> Self {
        self.full = true;
        self
    }

    pub fn show(&mut self, items: Vec<SharedString>, current: usize, cx: &mut Context<Self>) {
        if self.items != items || self.current != current {
            self.items = items;
            self.current = current;
            cx.notify();
        }
    }

    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        self.open = false;
        cx.emit(SelectEvent::Pick(index));
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" if self.open => self.open = false,
            "enter" | "space" => self.open = !self.open,
            "down" if self.current + 1 < self.items.len() => {
                return self.pick(self.current + 1, cx);
            }
            "up" if self.current > 0 => return self.pick(self.current - 1, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for Select {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        let label = self.items.get(self.current).cloned().unwrap_or_default();
        let button = div()
            .id(self.id)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .when_else(
                self.full,
                |button| button.w_full(),
                |button| button.w(px(WIDTH)),
            )
            .relative()
            .h(px(HEIGHT))
            .px(px(9.0))
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(6.0))
            .border_1()
            .border_color(if self.focus.is_focused(window) {
                c.accent
            } else {
                c.line_strong
            })
            .bg(c.bg)
            .text_size(px(TEXT))
            .text_color(c.ink)
            .cursor_pointer()
            .child(label)
            .child(icon(Lucide::ChevronDown, 14.0, c.ink_3))
            .when(self.full, |button| {
                button.child(measure(self.size.clone(), cx.entity_id()))
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.open = !this.open;
                window.focus(&this.focus, cx);
                cx.notify();
            }));
        let host = div()
            .relative()
            .when(self.full, Styled::w_full)
            .child(button);
        let list_w = if self.full {
            self.size.get().width
        } else {
            px(WIDTH)
        };
        if !self.open {
            return host;
        }
        let view = window.viewport_size();
        let backdrop = deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .anchor(Anchor::TopLeft)
                .child(div().w(view.width).h(view.height).occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open = false;
                        cx.notify();
                    }),
                )),
        )
        .with_priority(1);
        let rows = self
            .items
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, label)| {
                div()
                    .id(("select-row", index))
                    .h(px(ROW))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(if index == self.current {
                        c.accent_wash
                    } else {
                        c.surface
                    })
                    .hover(move |style| style.bg(c.surface_hover))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(index, cx)))
            });
        let list = deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .offset(point(px(0.0), px(HEIGHT + 2.0)))
                .snap_to_window()
                .child(
                    div()
                        .w(list_w)
                        .occlude()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(c.line_strong)
                        .bg(c.surface)
                        .text_size(px(TEXT))
                        .text_color(c.ink)
                        .children(rows),
                ),
        )
        .with_priority(2);
        host.child(backdrop).child(list)
    }
}
