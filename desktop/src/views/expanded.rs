use std::rc::Rc;

use gpui::{
    App, Div, Entity, FocusHandle, InteractiveElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, ScrollEvent, Styled, Window, div, px,
};

use crate::theme::Theme;
use crate::views::card;
use crate::views::terminal::TerminalView;

const PAD: f32 = 16.0;
const ROUND: f32 = 8.0;

pub fn overlay(
    face: &card::Face,
    selected: bool,
    terminal: Entity<TerminalView>,
    focus: &FocusHandle,
    collapse: Rc<dyn Fn(&mut App)>,
    room: f32,
    cx: &App,
) -> Div {
    let colors = cx.global::<Theme>().colors;
    let toggle = collapse.clone();
    let key_close = collapse;
    let focus_head = focus.clone();
    let head = card::head(face, 1.0, &colors).on_mouse_down(
        MouseButton::Left,
        move |e: &MouseDownEvent, window, cx| {
            window.focus(&focus_head, cx);
            if e.click_count == 2 {
                toggle(cx);
            }
        },
    );
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .p(px(PAD))
        .pb(px(PAD + room))
        .bg(colors.bg)
        .track_focus(focus)
        .on_key_down(move |e: &KeyDownEvent, _: &mut Window, cx| {
            if e.keystroke.key == "escape" {
                key_close(cx);
            }
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(ROUND))
                .border_1()
                .border_color(if selected {
                    colors.accent
                } else {
                    colors.line_strong
                })
                .bg(colors.term)
                .child(head)
                .child(div().flex_1().min_h(px(0.0)).child(terminal))
                .children(card::foot(face, selected, 1.0, &colors)),
        )
}
