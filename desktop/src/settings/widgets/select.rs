use gpui::{
    Anchor, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, point, px,
};

use crate::views::dock::ui::{TEXT, colors};

const HEIGHT: f32 = 30.0;
const WIDTH: f32 = 180.0;
const ROW: f32 = 28.0;

pub enum SelectEvent {
    Pick(usize),
}

pub struct Select {
    id: &'static str,
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
            items: Vec::new(),
            current: 0,
            open: false,
            focus: cx.focus_handle().tab_stop(true),
        }
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
            .w(px(WIDTH))
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
            .bg(c.surface)
            .text_size(px(TEXT))
            .text_color(c.ink)
            .cursor_pointer()
            .child(label)
            .child(div().text_color(c.ink_3).child("\u{25BE}"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open = !this.open;
                window.focus(&this.focus, cx);
                cx.notify();
            }));
        let host = div().relative().child(button);
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
                        .w(px(WIDTH))
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
