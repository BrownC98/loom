use gpui::{
    App, Div, ElementId, InteractiveElement, ParentElement, Stateful, StatefulInteractiveElement,
    Styled, Window, div, px,
};

use crate::views::dock::ui::colors;

const TRACK_W: f32 = 30.0;
const TRACK_H: f32 = 18.0;
const KNOB: f32 = 14.0;
const GAP: f32 = 2.0;

pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    change: impl Fn(bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let left = if on { TRACK_W - KNOB - GAP } else { GAP };
    div()
        .id(id)
        .relative()
        .flex_none()
        .self_start()
        .w(px(TRACK_W))
        .h(px(TRACK_H))
        .rounded(px(TRACK_H / 2.0))
        .cursor_pointer()
        .bg(if on { c.accent } else { c.line_strong })
        .child(
            div()
                .absolute()
                .top(px(GAP))
                .left(px(left))
                .size(px(KNOB))
                .rounded_full()
                .bg(if on { c.accent_ink } else { c.ink }),
        )
        .on_click(move |_, window, cx| change(!on, window, cx))
}
