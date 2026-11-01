use gpui::{
    App, ClickEvent, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::strings::Str;
use crate::views::dock::ui::{TEXT, button, colors, say};

const WIDTH: f32 = 380.0;

pub fn confirm(
    message: String,
    accept: &str,
    on_accept: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let c = colors(cx);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(c.scrim)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w(px(WIDTH))
                .p(px(18.0))
                .flex()
                .flex_col()
                .gap(px(16.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(c.line_strong)
                .bg(c.panel)
                .text_size(px(TEXT))
                .text_color(c.ink)
                .child(div().child(message))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            button("confirm-cancel", cx)
                                .child(say(cx, Str::CommonCancel))
                                .on_click(on_cancel),
                        )
                        .child(
                            button("confirm-accept", cx)
                                .bg(c.accent)
                                .text_color(c.accent_ink)
                                .child(accept.to_owned())
                                .on_click(on_accept),
                        ),
                ),
        )
}
