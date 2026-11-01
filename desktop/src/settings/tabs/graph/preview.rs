use api::graph::waits::WaitKind;
use api::settings::FloatSize;
use gpui::{
    App, BoxShadow, Div, FontWeight, IntoElement, ParentElement, Styled, Window, canvas, div, fill,
    point, px, size,
};

use crate::strings::Str;
use crate::views::dock::questions::kind_label;
use crate::views::dock::ui::{SMALL, colors, say};

const DOT_GAP: f32 = 20.0;
const HEAD: f32 = 34.0;
const PILL_ID: &str = "1";
const COMPOSER_W: f32 = 440.0;
const BUTTON: f32 = 30.0;
const SHADOW_BLUR: f32 = 14.0;
const SHADOW_DROP: f32 = 4.0;

pub fn zoom(size: FloatSize) -> f32 {
    match size {
        FloatSize::Small => 0.85,
        FloatSize::Normal => 1.0,
        FloatSize::Large => 1.15,
        FloatSize::Huge => 1.3,
    }
}

fn shadow(color: gpui::Rgba) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.into(),
        offset: point(px(0.0), px(SHADOW_DROP)),
        blur_radius: px(SHADOW_BLUR),
        spread_radius: px(0.0),
        inset: false,
    }]
}

fn tool(glyph: &'static str, z: f32, cx: &App) -> Div {
    div()
        .flex_none()
        .size(px(24.0 * z))
        .flex()
        .items_center()
        .justify_center()
        .text_color(colors(cx).ink_3)
        .child(glyph)
}

pub fn pill(size: FloatSize, name: &str, text: &str, cx: &App) -> Div {
    let c = colors(cx);
    let z = zoom(size);
    let head = div()
        .flex()
        .items_center()
        .gap(px(8.0 * z))
        .min_w_0()
        .h(px(HEAD * z))
        .px(px(6.0 * z))
        .text_size(px(13.0 * z))
        .child(tool("\u{00d7}", z, cx))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::BOLD)
                .child(say(cx, kind_label(WaitKind::Question))),
        )
        .child(
            div()
                .flex_none()
                .min_w(px(18.0 * z))
                .h(px(18.0 * z))
                .px(px(5.0 * z))
                .rounded(px(9.0 * z))
                .bg(c.warn)
                .text_color(c.warn_ink)
                .text_size(px(11.0 * z))
                .flex()
                .items_center()
                .justify_center()
                .child("?1"),
        )
        .child(
            div()
                .flex_none()
                .px(px(6.0 * z))
                .h(px(18.0 * z))
                .rounded(px(4.0 * z))
                .border_1()
                .border_color(c.warn)
                .text_size(px(11.0 * z))
                .flex()
                .items_center()
                .child(PILL_ID),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(4.0 * z))
                .py(px(2.0 * z))
                .text_size(px(12.0 * z))
                .truncate()
                .child(format!("{name}: {text}")),
        );
    div()
        .flex()
        .flex_col()
        .rounded(px(17.0 * z))
        .border_1()
        .border_color(c.warn)
        .bg(c.panel)
        .text_color(c.ink)
        .shadow(shadow(c.shadow))
        .child(head)
}

pub fn composer(size: FloatSize, name: &str, cx: &App) -> Div {
    let c = colors(cx);
    let z = zoom(size);
    let button = |glyph: &'static str| {
        div()
            .flex_none()
            .size(px(BUTTON * z))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0 * z))
            .text_color(c.ink_2)
            .child(glyph)
    };
    let to = div()
        .flex()
        .items_center()
        .gap(px(6.0 * z))
        .min_w_0()
        .text_size(px(12.0 * z))
        .text_color(c.ink_3)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0 * z))
                .child(">_")
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.ink)
                        .child(name.to_owned()),
                )
                .child(say(cx, Str::ComposerTo)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .child(say(cx, Str::ComposerKeys)),
        );
    let line = div()
        .flex()
        .items_end()
        .gap(px(4.0 * z))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(8.0 * z))
                .py(px(5.0 * z))
                .h(px(30.0 * z))
                .rounded(px(8.0 * z))
                .border_1()
                .border_color(c.line_strong)
                .bg(c.surface)
                .text_size(px(13.0 * z))
                .text_color(c.ink_3)
                .child(say(cx, Str::ComposerPlaceholder)),
        )
        .child(button("\u{25a3}"))
        .child(button("+"))
        .child(
            div()
                .flex_none()
                .size(px(BUTTON * z))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0 * z))
                .bg(c.accent)
                .text_color(c.accent_ink)
                .opacity(0.4)
                .child("\u{2191}"),
        );
    div()
        .flex()
        .flex_col()
        .gap(px(6.0 * z))
        .w(px(COMPOSER_W * z))
        .py(px(8.0 * z))
        .pl(px(10.0 * z))
        .pr(px(8.0 * z))
        .rounded(px(14.0 * z))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.panel)
        .shadow(shadow(c.shadow))
        .text_size(px(13.0 * z))
        .text_color(c.ink)
        .child(to)
        .child(line)
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
