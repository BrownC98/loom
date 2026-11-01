use gpui::{
    App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::btn::tabbable;
use crate::views::dock::ui::colors;

const TRACK_W: f32 = 30.0;
const TRACK_H: f32 = 18.0;
const KNOB: f32 = 14.0;
const GAP: f32 = 2.0;
const LABEL_GAP: f32 = 8.0;

pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    label: impl IntoElement,
    change: impl Fn(bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let left = if on { TRACK_W - KNOB - GAP } else { GAP };
    let track = div()
        .relative()
        .flex_none()
        .w(px(TRACK_W))
        .h(px(TRACK_H))
        .rounded(px(TRACK_H / 2.0))
        .bg(if on { c.accent } else { c.line_strong })
        .child(
            div()
                .absolute()
                .top(px(GAP))
                .left(px(left))
                .size(px(KNOB))
                .rounded_full()
                .bg(if on { c.accent_ink } else { c.ink }),
        );
    tabbable(div().id(id), cx)
        .flex()
        .items_center()
        .gap(px(LABEL_GAP))
        .self_start()
        .cursor_pointer()
        .child(track)
        .child(label)
        .on_click(move |_, window, cx| change(!on, window, cx))
}
