use gpui::{
    App, Bounds, Div, InteractiveElement, IntoElement, MouseButton, ParentElement, Rgba, Styled,
    Window, canvas, div, fill, point, px, size,
};

use crate::platform;
use crate::theme::Colors;

pub const HEIGHT: f32 = 36.0;
const MARK: f32 = 16.0;
const CELLS: usize = 3;
const PITCH: f32 = 5.0;
const LONG: f32 = 4.0;
const SHORT: f32 = 2.0;

fn mark(color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for row in 0..CELLS {
                for col in 0..CELLS {
                    let x = 0.5 + col as f32 * PITCH;
                    let y = 0.5 + row as f32 * PITCH;
                    let (dx, dy, w, h) = if (row + col) % 2 == 0 {
                        (0.0, 1.0, LONG, SHORT)
                    } else {
                        (1.0, 0.0, SHORT, LONG)
                    };
                    let cell = Bounds::new(
                        bounds.origin + point(px(x + dx), px(y + dy)),
                        size(px(w), px(h)),
                    );
                    window.paint_quad(fill(cell, color));
                }
            }
        },
    )
    .w(px(MARK))
    .h(px(MARK))
}

pub fn bar(
    window: &Window,
    colors: &Colors,
    settings: &'static str,
    open: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let hover = colors.surface_hover;
    platform::drag_area(
        div()
            .h(px(HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(platform::leading_gap())
            .bg(colors.panel)
            .border_b_1()
            .border_color(colors.line)
            .text_color(colors.ink_2)
            .text_size(px(13.0))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count >= 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(div().mx(px(12.0)).child(mark(colors.ink_2)))
            .child(div().text_color(colors.ink).child("loom"))
            .child(div().flex_1())
            .child(
                div()
                    .id("title-settings")
                    .px(px(10.0))
                    .py(px(4.0))
                    .mr(px(8.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        open(window, cx);
                    })
                    .child(settings),
            )
            .children(platform::captions(window, colors)),
    )
}
