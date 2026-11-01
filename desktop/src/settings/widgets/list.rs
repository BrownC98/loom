use std::rc::Rc;

use gpui::{
    App, Div, ElementId, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::views::dock::ui::{TEXT, colors};

const ROW: f32 = 28.0;

pub fn list_box(
    id: &'static str,
    items: Vec<SharedString>,
    selected: Option<usize>,
    width: f32,
    height: f32,
    change: impl Fn(usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let change = Rc::new(change);
    let rows = items.into_iter().enumerate().map(|(index, label)| {
        let change = change.clone();
        let chosen = selected == Some(index);
        div()
            .id(ElementId::NamedInteger(id.into(), index as u64))
            .flex_none()
            .h(px(ROW))
            .px(px(10.0))
            .flex()
            .items_center()
            .text_size(px(TEXT))
            .text_color(c.ink)
            .cursor_pointer()
            .bg(if chosen { c.accent_wash } else { c.surface })
            .hover(move |style| style.bg(c.surface_hover))
            .child(label)
            .on_click(move |_, window, cx| change(index, window, cx))
    });
    div()
        .id(id)
        .w(px(width))
        .h(px(height))
        .flex_none()
        .overflow_y_scroll()
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong)
        .flex()
        .flex_col()
        .children(rows)
}
