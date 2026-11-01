use chrono::{Local, TimeZone, Timelike};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Stateful, StatefulInteractiveElement, Styled, div, px,
};

use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::field::Field;
use crate::views::scrollbar::Bar;

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

pub fn day_and_time(seconds: i64, lang: Lang) -> String {
    let Some(time) = Local.timestamp_opt(seconds, 0).single() else {
        return String::new();
    };
    let (hour, minute) = (time.hour(), time.minute());
    let (month, day) = (time.format("%-m"), time.format("%-d"));
    let twelve = if hour % 12 == 0 { 12 } else { hour % 12 };
    match lang {
        Lang::En => {
            let part = if hour < 12 { "AM" } else { "PM" };
            format!("{month}/{day}, {twelve}:{minute:02}\u{202f}{part}")
        }
        Lang::Ko => {
            let part = if hour < 12 { "오전" } else { "오후" };
            format!("{month}. {day}. {part} {twelve}:{minute:02}")
        }
    }
}

pub fn ago(seconds: i64, lang: Lang) -> String {
    const STEPS: [(i64, usize); 6] = [
        (365 * 86_400, 0),
        (30 * 86_400, 1),
        (7 * 86_400, 2),
        (86_400, 3),
        (3_600, 4),
        (60, 5),
    ];
    for (size, unit) in STEPS {
        if seconds >= size {
            let count = (seconds as f64 / size as f64 - 0.5).ceil() as i64;
            return ago_text(unit, count, lang);
        }
    }
    match lang {
        Lang::En => "now",
        Lang::Ko => "지금",
    }
    .to_owned()
}

fn ago_text(unit: usize, count: i64, lang: Lang) -> String {
    match lang {
        Lang::En => {
            const LAST: [Option<&str>; 6] = [
                Some("last year"),
                Some("last month"),
                Some("last week"),
                Some("yesterday"),
                None,
                None,
            ];
            const NAME: [&str; 6] = ["year", "month", "week", "day", "hour", "minute"];
            match LAST[unit] {
                Some(last) if count == 1 => last.to_owned(),
                _ if count == 1 => format!("1 {} ago", NAME[unit]),
                _ => format!("{count} {}s ago", NAME[unit]),
            }
        }
        Lang::Ko => {
            const LAST: [Option<&str>; 6] = [
                Some("작년"),
                Some("지난달"),
                Some("지난주"),
                Some("어제"),
                None,
                None,
            ];
            const NAME: [&str; 6] = ["년", "개월", "주", "일", "시간", "분"];
            match LAST[unit] {
                Some(last) if count == 1 => last.to_owned(),
                _ => format!("{count}{} 전", NAME[unit]),
            }
        }
    }
}

pub fn open_url(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|e| e.to_string())
}

pub fn button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    button_at(id, 1.0, cx)
}

pub fn button_at(id: impl Into<ElementId>, zoom: f32, cx: &App) -> Stateful<Div> {
    let c = colors(cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(GAP * zoom))
        .min_h(px(26.0 * zoom))
        .px(px(10.0 * zoom))
        .py(px(3.0 * zoom))
        .rounded(px(6.0 * zoom))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.surface)
        .text_size(px(SMALL * zoom))
        .text_color(c.ink)
        .cursor_pointer()
        .hover(move |style| style.bg(c.surface_hover))
}

pub fn note(text: String, error: bool, cx: &App) -> Div {
    let c = colors(cx);
    let base = div().p(px(16.0));
    if error {
        base.text_size(px(SMALL))
            .text_color(c.err)
            .font_family(crate::fonts::TERMINAL_FAMILY)
            .child(text)
    } else {
        base.text_size(px(TEXT)).text_color(c.ink_3).child(text)
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
        .child(text.to_owned())
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

pub fn count(n: usize, c: &Colors) -> Div {
    div()
        .flex_none()
        .min_w(px(18.0))
        .h(px(18.0))
        .px(px(5.0))
        .rounded(px(9.0))
        .bg(c.notice)
        .text_color(c.notice_ink)
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(n.to_string())
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
