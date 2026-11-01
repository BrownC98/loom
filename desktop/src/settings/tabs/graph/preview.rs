use api::graph::waits::{WaitKind, WaitTone};
use api::settings::FloatSize;
use gpui::{
    App, Div, IntoElement, ParentElement, Styled, Window, canvas, div, fill, point, px, size,
};

use crate::float::composer::{self, ComposerView, ComposerWire};
use crate::float::pill::{self, PillView, PillWire};
use crate::float::place;
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, colors, say};
use crate::views::field;

const DOT_GAP: f32 = 20.0;
const PILL_ID: &str = "1";
const COMPOSER_W: f32 = 440.0;

struct Still;

impl PillWire for Still {}
impl ComposerWire for Still {}

pub fn pill(size: FloatSize, name: &str, text: &str, cx: &App) -> Div {
    let z = place::zoom(size);
    let view = PillView {
        name,
        summary: text,
        kind: WaitKind::Question,
        tone: WaitTone::Ask,
        id: PILL_ID.to_owned(),
        asks: 1,
        dones: 0,
        line: true,
        open: false,
        reset: false,
        z,
    };
    pill::face(&view, None, &Still, cx).max_w(px(pill::WIDTH * z))
}

pub fn composer(size: FloatSize, name: &str, cx: &App) -> Div {
    let z = place::zoom(size);
    let c = colors(cx);
    let view = ComposerView {
        name,
        home: true,
        z,
        focused: false,
        sendable: true,
        listing: false,
        filing: None,
        note: None,
    };
    let input = field::skin(div(), z, false, &c)
        .text_color(c.ink_3)
        .child(say(cx, Str::ComposerPlaceholder))
        .into_any_element();
    composer::face(&view, None, input, &Still, cx).w(px(COMPOSER_W * z))
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
