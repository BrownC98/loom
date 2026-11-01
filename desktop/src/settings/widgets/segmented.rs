use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, FontWeight, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::views::dock::ui::{TEXT, colors};

pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    items: Vec<(T, SharedString)>,
    current: T,
    change: impl Fn(T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let change = Rc::new(change);
    let last = items.len().saturating_sub(1);
    let row = div()
        .id(id)
        .flex()
        .flex_none()
        .self_start()
        .overflow_hidden()
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong);
    items
        .into_iter()
        .enumerate()
        .fold(row, |row, (index, (value, label))| {
            let chosen = value == current;
            let change = change.clone();
            let item = div()
                .id(ElementId::NamedInteger(id.into(), index as u64))
                .px(px(14.0))
                .py(px(5.0))
                .text_size(px(TEXT))
                .cursor_pointer()
                .border_color(c.line_strong)
                .when(index != last, |item| item.border_r_1())
                .text_color(c.ink)
                .font_weight(if chosen {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .bg(if chosen { c.accent_wash } else { c.surface })
                .child(label)
                .on_click(move |_, window, cx| change(value, window, cx));
            row.child(item)
        })
}
