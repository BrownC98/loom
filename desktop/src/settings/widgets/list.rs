use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::views::dock::ui::{TEXT, colors};
use crate::views::scrollbar::Bar;

const ROW: f32 = 24.0;
const GAP: f32 = 1.0;
pub const PITCH: f32 = ROW + GAP;

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
            .px(px(8.0))
            .rounded(px(5.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(TEXT))
            .text_color(if chosen { c.ink } else { c.ink_2 })
            .font_weight(if chosen {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .cursor_pointer()
            .when(chosen, |row| row.bg(c.accent_wash))
            .hover(move |style| style.bg(c.surface_hover).text_color(c.ink))
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
        .track_scroll(scroll.handle())
        .p(px(4.0))
        .flex()
        .flex_col()
        .gap(px(GAP))
        .when_some(hover.clone(), |list, hover| {
            list.on_hover(move |inside, window, cx| {
                if !*inside {
                    hover(None, window, cx);
                }
            })
        })
        .children(rows);
    scroll
        .on_overflow(area)
        .w(px(width))
        .h(px(height))
        .flex_none()
        .rounded(px(8.0))
        .border_1()
        .border_color(c.line)
        .bg(c.bg)
        .overflow_hidden()
}
