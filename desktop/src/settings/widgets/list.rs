use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::views::dock::ui::{TEXT, colors};
use crate::views::scrollbar::Bar;

const ROW: f32 = 28.0;

#[allow(clippy::too_many_arguments)]
pub fn list_box_on(
    id: &'static str,
    items: Vec<(SharedString, Option<AnyElement>)>,
    selected: Option<usize>,
    width: f32,
    height: f32,
    change: impl Fn(usize, &mut Window, &mut App) + 'static,
    hover: Option<impl Fn(Option<usize>, &mut Window, &mut App) + 'static>,
    scroll: &Bar,
    cx: &App,
) -> Div {
    let c = colors(cx);
    let change = Rc::new(change);
    let hover = hover.map(Rc::new);
    let rows = items.into_iter().enumerate().map(|(index, (label, lead))| {
        let change = change.clone();
        let hover = hover.clone();
        let chosen = selected == Some(index);
        div()
            .id(ElementId::NamedInteger(id.into(), index as u64))
            .flex_none()
            .h(px(ROW))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(TEXT))
            .text_color(c.ink)
            .cursor_pointer()
            .bg(if chosen { c.accent_wash } else { c.surface })
            .hover(move |style| style.bg(c.surface_hover))
            .children(lead)
            .child(label)
            .when_some(hover, |row, hover| {
                row.on_hover(move |inside, window, cx| {
                    if *inside {
                        hover(Some(index), window, cx);
                    }
                })
            })
            .on_click(move |_, window, cx| change(index, window, cx))
    });
    let area = div()
        .id(id)
        .overflow_y_scroll()
        .track_scroll(&scroll.handle())
        .flex()
        .flex_col()
        .when_some(hover.clone(), |list, hover| {
            list.on_hover(move |inside, window, cx| {
                if !*inside {
                    hover(None, window, cx);
                }
            })
        })
        .children(rows);
    scroll
        .wrap(area, false)
        .w(px(width))
        .h(px(height))
        .flex_none()
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong)
        .overflow_hidden()
}
