use api::settings::FloatSize;
use gpui::{App, Div, IntoElement, ParentElement, Styled, Window, canvas, div, fill, point, size};

use crate::float::aim::Kind;
use crate::float::composer::{self, ComposerView, ComposerWire};
use crate::float::place;
use crate::scale::{self, px};
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, TEXT, colors, say};
use crate::views::field;

const DOT_GAP: f32 = 20.0;
const COMPOSER_W: f32 = 440.0;

struct Still;

impl ComposerWire for Still {}

pub fn composer(size: FloatSize, name: &str, cx: &App) -> Div {
    let z = place::zoom(size);
    let c = colors(cx);
    let view = ComposerView {
        name,
        kind: Kind::Prompt,
        home: true,
        z,
        focused: false,
        sendable: true,
        listing: false,
        keys: None,
        filing: None,
        note: None,
        closable: false,
    };
    let input = field::skin(div(), z, false, &c)
        .text_color(c.ink_3)
        .child(say(cx, Str::ComposerPlaceholder))
        .into_any_element();
    composer::face(&view, None, None, input, &Still, cx).w(scale::fixed(COMPOSER_W * z))
}

pub fn ui(size: FloatSize, rows: [&str; 2], ask: &str, acts: [&str; 2], cx: &App) -> Div {
    let z = place::zoom(size);
    let c = colors(cx);
    let at = |value: f32| scale::fixed(value * z);
    let row = |name: &str, dot: gpui::Rgba| {
        div()
            .flex()
            .items_center()
            .gap(at(6.0))
            .px(at(6.0))
            .h(at(26.0))
            .text_size(at(TEXT))
            .text_color(c.ink)
            .child(div().flex_none().size(at(7.0)).rounded_full().bg(dot))
            .child(name.to_owned())
    };
    let list = div()
        .flex_none()
        .w(at(140.0))
        .p(at(6.0))
        .rounded(at(8.0))
        .border_1()
        .border_color(c.line)
        .bg(c.panel)
        .child(row(rows[0], c.ok))
        .child(row(rows[1], c.ink_3));
    let act = |text: &str| {
        div()
            .flex()
            .items_center()
            .h(at(24.0))
            .px(at(8.0))
            .rounded(at(6.0))
            .border_1()
            .border_color(c.line_strong)
            .bg(c.surface)
            .text_size(at(SMALL))
            .child(text.to_owned())
    };
    let card = div()
        .flex_none()
        .w(at(220.0))
        .flex()
        .flex_col()
        .gap(at(6.0))
        .p(at(10.0))
        .rounded(at(8.0))
        .border_1()
        .border_color(c.warn)
        .bg(c.surface)
        .text_size(at(SMALL))
        .text_color(c.ink)
        .child(
            div()
                .flex()
                .items_center()
                .gap(at(6.0))
                .child(div().text_color(c.ink_3).child("Q1"))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(rows[0].to_owned()),
                )
                .child(div().flex_1())
                .child(div().text_color(c.ink_3).child("12:00")),
        )
        .child(ask.to_owned())
        .child(
            div()
                .flex()
                .gap(at(6.0))
                .child(act(&format!("A {}", acts[0])))
                .child(act(&format!("B {}", acts[1]))),
        );
    div()
        .flex()
        .items_start()
        .gap(at(12.0))
        .child(list)
        .child(card)
}

pub fn frame(caption: impl IntoElement, shown: Div, room: Div, cx: &App) -> Div {
    let c = colors(cx);
    let dot = c.dot;
    let dots = canvas(
        |_, _, _| (),
        move |bounds, _, window: &mut Window, _| {
            let mut y = bounds.origin.y;
            while y < bounds.origin.y + bounds.size.height {
                let mut x = bounds.origin.x;
                while x < bounds.origin.x + bounds.size.width {
                    window.paint_quad(fill(
                        gpui::Bounds::new(point(x, y), size(px(1.0), px(1.0))),
                        dot,
                    ));
                    x += px(DOT_GAP);
                }
                y += px(DOT_GAP);
            }
        },
    )
    .absolute()
    .size_full();
    div()
        .relative()
        .overflow_hidden()
        .max_w(px(720.0))
        .px(px(16.0))
        .pt(px(22.0))
        .pb(px(10.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(c.line)
        .bg(c.bg)
        .child(dots)
        .child(
            div()
                .absolute()
                .left(px(10.0))
                .top(px(4.0))
                .text_size(px(SMALL - 1.0))
                .text_color(c.ink_3)
                .child(caption),
        )
        .child(
            div()
                .relative()
                .child(div().invisible().child(room))
                .child(div().absolute().top_0().left_0().child(shown)),
        )
}
