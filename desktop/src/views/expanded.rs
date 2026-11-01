use std::rc::Rc;

use api::graph::NodeView;
use gpui::{
    App, Div, Entity, FocusHandle, InteractiveElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, ScrollEvent, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::strings::{Lang, Str};
use crate::theme::Theme;
use crate::views::card;
use crate::views::terminal::TerminalView;

const PAD: f32 = 16.0;
const HEAD: f32 = 32.0;
const ROUND: f32 = 8.0;
const TEXT: f32 = 14.0;
const SMALL: f32 = 12.0;

pub fn overlay(
    node: &NodeView,
    terminal: Entity<TerminalView>,
    focus: &FocusHandle,
    collapse: Rc<dyn Fn(&mut App)>,
    room: f32,
    cx: &App,
) -> Div {
    let colors = cx.global::<Theme>().colors;
    let lang = *cx.global::<Lang>();
    let close = collapse.clone();
    let toggle = collapse.clone();
    let key_close = collapse;
    let focus_head = focus.clone();
    let button = div()
        .id("expanded-collapse")
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(colors.line_strong)
        .bg(colors.surface)
        .text_size(px(SMALL))
        .text_color(colors.ink)
        .cursor_pointer()
        .hover(move |style| style.bg(colors.surface_hover))
        .on_click(move |_, _, cx| close(cx))
        .child(Str::NodeCollapse.text(lang));
    let head = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .h(px(HEAD))
        .px(px(12.0))
        .border_b_1()
        .border_color(colors.line)
        .text_size(px(TEXT))
        .text_color(colors.ink)
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
            window.focus(&focus_head, cx);
            if e.click_count == 2 {
                toggle(cx);
            }
        })
        .child(node.name.clone())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(colors.ink_3)
                        .child(format!("#{} {}", node.id, card::state(&node.state))),
                )
                .child(button),
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
                .border_color(colors.line_strong)
                .bg(colors.surface)
                .child(head)
                .child(div().flex_1().min_h(px(0.0)).child(terminal)),
        )
}
