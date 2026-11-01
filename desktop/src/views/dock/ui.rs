pub use crate::views::ui::{
    GAP, PAD, SMALL, TEXT, ago, button, button_at, clock, colors, count, day_and_time, open_url,
    say, say_with,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::views::field::Field;
use crate::views::prose::prose;
use crate::views::scrollbar::Bar;

pub fn note(text: String, error: bool, cx: &App) -> Div {
    let c = colors(cx);
    let base = div().p(px(16.0));
    if error {
        base.text_size(px(SMALL))
            .text_color(c.err)
            .font_family(crate::fonts::TERMINAL_FAMILY)
            .child(prose(text))
    } else {
        base.text_size(px(TEXT))
            .text_color(c.ink_3)
            .child(prose(text))
    }
}

pub fn hint(text: &str, cx: &App) -> Div {
    let c = colors(cx);
    div()
        .flex_none()
        .px(px(16.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(c.line)
        .text_size(px(SMALL))
        .text_color(c.ink_3)
        .child(prose(text.to_owned()))
}

pub fn filters(cx: &App) -> Div {
    let c = colors(cx);
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(GAP))
        .px(px(16.0))
        .py(px(10.0))
        .border_b_1()
        .border_color(c.line)
}

pub fn chip(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let c = colors(cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.0))
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(13.0))
        .border_1()
        .border_color(if on { c.accent } else { c.line_strong })
        .when(on, |chip| chip.bg(c.accent_wash))
        .text_size(px(SMALL))
        .text_color(if on { c.ink } else { c.ink_2 })
        .cursor_pointer()
        .hover(move |style| style.bg(c.surface_hover))
}

pub fn search(field: &Entity<Field>, cx: &App) -> Div {
    let c = colors(cx);
    div()
        .track_focus(&field.read(cx).handle())
        .ml_auto()
        .flex_none()
        .w(px(160.0))
        .h(px(26.0))
        .px(px(9.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.bg)
        .text_size(px(SMALL))
        .text_color(c.ink)
        .focus(move |style| style.border_color(c.accent))
        .child(field.clone())
}

pub fn empty(text: String, error: bool, cx: &App) -> Div {
    let c = colors(cx);
    div()
        .px(px(14.0))
        .py(px(12.0))
        .text_size(px(TEXT))
        .text_color(if error { c.err } else { c.ink_3 })
        .child(text)
}

pub fn scroller(bar: &Bar, rows: impl IntoElement) -> Div {
    bar.reserved(
        div()
            .id("dock-panel")
            .overflow_y_scroll()
            .track_scroll(bar.handle())
            .child(rows),
    )
    .flex_1()
    .min_h_0()
}
