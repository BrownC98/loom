use gpui::{
    Anchor, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, Styled, Window, anchored, deferred, div, point, px,
};

pub struct Closed;

const EDGE: f32 = 8.0;

pub fn overlay(
    at: Point<Pixels>,
    body: impl IntoElement,
    outside: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    window: &Window,
) -> Div {
    let view = window.viewport_size();
    let backdrop = deferred(
        anchored()
            .position(point(px(0.0), px(0.0)))
            .anchor(Anchor::TopLeft)
            .child(
                div()
                    .w(view.width)
                    .h(view.height)
                    .on_mouse_down(MouseButton::Left, outside),
            ),
    )
    .with_priority(1);
    let shown = deferred(
        anchored()
            .position(at)
            .anchor(Anchor::TopLeft)
            .snap_to_window_with_margin(px(EDGE))
            .child(
                div()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .child(body),
            ),
    )
    .with_priority(2);
    div().child(backdrop).child(shown)
}
