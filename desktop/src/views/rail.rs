use gpui::{
    Div, ElementId, InteractiveElement, ParentElement, Stateful, StatefulInteractiveElement,
    Styled, div, px,
};

use super::icons::{Lucide, icon};
use super::tooltip::tip;
use crate::theme::Colors;

pub const BUTTON: f32 = 30.0;
pub const SMALL_BUTTON: f32 = 26.0;
const ICON: f32 = 14.0;

pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘⇧{key}")
    } else {
        format!("Ctrl+Shift+{key}")
    }
}

pub fn titled(label: &str, key: &str) -> String {
    format!("{label} ({})", shortcut(key))
}

pub fn column(c: &Colors) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0))
        .py(px(8.0))
        .bg(c.panel)
}

pub fn button(
    id: impl Into<ElementId>,
    glyph: Lucide,
    label: String,
    size: f32,
    c: &Colors,
) -> Stateful<Div> {
    let hover = c.surface_hover;
    div()
        .id(id)
        .relative()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .tooltip(tip(label))
        .child(icon(glyph, ICON, c.ink_2))
}

pub fn divider(c: &Colors) -> Div {
    div()
        .w(px(18.0))
        .h(px(1.0))
        .flex_none()
        .my(px(4.0))
        .bg(c.line_strong)
}

pub fn badge(count: usize, c: &Colors) -> Div {
    div()
        .absolute()
        .top(px(-3.0))
        .right(px(-2.0))
        .min_w(px(15.0))
        .h(px(15.0))
        .px(px(3.0))
        .rounded(px(8.0))
        .bg(c.notice)
        .text_color(c.notice_ink)
        .text_size(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .child(count.to_string())
}

pub fn dot(c: &Colors) -> Div {
    div()
        .absolute()
        .top(px(5.0))
        .right(px(5.0))
        .size(px(6.0))
        .rounded_full()
        .bg(c.warn)
}
