use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Styled, Window, deferred, div, px,
};

use crate::theme::Colors;
use crate::views::icons::{Lucide, icon};

pub const WIDTH: f32 = 180.0;
const PAD: f32 = 6.0;
const ROW: f32 = 30.0;
const ROUND: f32 = 10.0;
const ROW_ROUND: f32 = 6.0;
const TEXT: f32 = 13.0;
const ICON: f32 = 14.0;

pub type Run = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Item {
    pub icon: Lucide,
    pub label: String,
    pub danger: bool,
    pub disabled: bool,
    pub run: Run,
}

pub enum Entry {
    Item(Item),
    Rule,
}

pub fn menu(
    id: impl Into<ElementId>,
    entries: Vec<Entry>,
    at: Point<Pixels>,
    focus: usize,
    c: &Colors,
) -> Div {
    let (surface, hover, ink, ink_3, err) = (c.surface, c.surface_hover, c.ink, c.ink_3, c.err);
    let mut live = 0usize;
    let rows = entries
        .into_iter()
        .enumerate()
        .map(|(i, entry)| match entry {
            Entry::Rule => div()
                .h(px(1.0))
                .mx(px(2.0))
                .my(px(4.0))
                .bg(c.line)
                .into_any_element(),
            Entry::Item(item) => {
                let color = if item.disabled {
                    ink_3
                } else if item.danger {
                    err
                } else {
                    ink
                };
                let run = item.run;
                let disabled = item.disabled;
                let focused = !disabled && live == focus;
                if !disabled {
                    live += 1;
                }
                div()
                    .id(("menu-item", i))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(ROW))
                    .px(px(8.0))
                    .rounded(px(ROW_ROUND))
                    .when(focused, |row| row.bg(hover))
                    .text_size(px(TEXT))
                    .text_color(color)
                    .when(!disabled, |row| {
                        row.cursor_pointer().hover(move |s| s.bg(hover))
                    })
                    .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
                        if !disabled {
                            run(window, cx);
                        }
                        cx.stop_propagation();
                    })
                    .child(icon(item.icon, ICON, color))
                    .child(item.label)
                    .into_any_element()
            }
        });
    let panel = div()
        .id(id)
        .absolute()
        .left(at.x)
        .top(at.y)
        .w(px(WIDTH))
        .p(px(PAD))
        .flex()
        .flex_col()
        .rounded(px(ROUND))
        .bg(surface)
        .shadow_lg()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .children(rows);
    div()
        .absolute()
        .size_full()
        .child(deferred(panel).with_priority(2))
}
