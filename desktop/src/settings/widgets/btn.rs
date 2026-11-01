use gpui::{
    App, BoxShadow, Div, ElementId, InteractiveElement, Stateful, StatefulInteractiveElement,
    Styled, point, px, transparent_black,
};

use crate::views::dock::ui::{self, colors};

const RING: f32 = 2.0;

pub fn tabbable(element: Stateful<Div>, cx: &App) -> Stateful<Div> {
    let c = colors(cx);
    element
        .focusable()
        .tab_stop(true)
        .focus_visible(move |style| {
            style.shadow(vec![BoxShadow {
                color: c.accent.into(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(RING),
                inset: false,
            }])
        })
}

pub fn inert(element: Stateful<Div>) -> Stateful<Div> {
    element.tab_stop(false)
}

pub fn button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    tabbable(ui::button(id, cx), cx)
}

pub fn ghost(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let c = colors(cx);
    button(id, cx)
        .border_color(transparent_black())
        .bg(c.panel)
        .text_color(c.ink_2)
}
