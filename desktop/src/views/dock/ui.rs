use chrono::{Local, TimeZone};
use gpui::{App, Div, ElementId, InteractiveElement, ParentElement, Stateful, Styled, div, px};

use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};

pub const TEXT: f32 = 13.0;
pub const SMALL: f32 = 12.0;
pub const PAD: f32 = 8.0;
pub const GAP: f32 = 6.0;

pub fn colors(cx: &App) -> Colors {
    cx.global::<Theme>().colors
}

pub fn say(cx: &App, text: Str) -> &'static str {
    text.text(*cx.global::<Lang>())
}

pub fn say_with(cx: &App, text: Str, args: &[(&str, &str)]) -> String {
    text.with(*cx.global::<Lang>(), args)
}

pub fn clock(seconds: i64, format: &str) -> String {
    Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|time| time.format(format).to_string())
        .unwrap_or_default()
}

pub fn open_url(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|e| e.to_string())
}

pub fn button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let c = colors(cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(GAP))
        .min_h(px(26.0))
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.surface)
        .text_size(px(SMALL))
        .text_color(c.ink)
        .cursor_pointer()
        .hover(move |style| style.bg(c.surface_hover))
}

pub fn link(id: impl Into<ElementId>, label: String, cx: &App) -> Stateful<Div> {
    let c = colors(cx);
    div()
        .id(id)
        .text_color(c.accent)
        .underline()
        .cursor_pointer()
        .child(label)
}

pub fn note(text: String, error: bool, cx: &App) -> Div {
    let c = colors(cx);
    div()
        .px(px(PAD))
        .py(px(GAP))
        .text_size(px(SMALL))
        .text_color(if error { c.err } else { c.ink_3 })
        .child(text)
}
