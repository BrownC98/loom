use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, InteractiveElement, ParentElement, ScrollHandle, SharedString, Stateful,
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
    list_box_on(
        id,
        items,
        selected,
        width,
        height,
        change,
        None::<fn(Option<usize>, &mut Window, &mut App)>,
        None,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn list_box_on(
    id: &'static str,
    items: Vec<SharedString>,
    selected: Option<usize>,
    width: f32,
    height: f32,
    change: impl Fn(usize, &mut Window, &mut App) + 'static,
    hover: Option<impl Fn(Option<usize>, &mut Window, &mut App) + 'static>,
    scroll: Option<&ScrollHandle>,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let change = Rc::new(change);
    let hover = hover.map(Rc::new);
    let rows = items.into_iter().enumerate().map(|(index, label)| {
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
            .text_size(px(TEXT))
            .text_color(c.ink)
            .cursor_pointer()
            .bg(if chosen { c.accent_wash } else { c.surface })
            .hover(move |style| style.bg(c.surface_hover))
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
    div()
        .id(id)
        .w(px(width))
        .h(px(height))
        .flex_none()
        .overflow_y_scroll()
        .when_some(scroll, |list, scroll| list.track_scroll(scroll))
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong)
        .flex()
        .flex_col()
        .when_some(hover.clone(), |list, hover| {
            list.on_hover(move |inside, window, cx| {
                if !*inside {
                    hover(None, window, cx);
                }
            })
        })
        .children(rows)
}
