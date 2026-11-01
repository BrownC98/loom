use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, FontWeight, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::views::dock::ui::{TEXT, colors};
use crate::views::sidebar::icons::{Glyph, icon};

pub struct Label {
    text: SharedString,
    glyph: Option<Glyph>,
}

impl Label {
    pub fn with_icon(text: SharedString, glyph: Glyph) -> Self {
        Self {
            text,
            glyph: Some(glyph),
        }
    }
}

impl From<SharedString> for Label {
    fn from(text: SharedString) -> Self {
        Self { text, glyph: None }
    }
}

pub fn segmented<T: Copy + PartialEq + 'static, L: Into<Label>>(
    id: &'static str,
    items: Vec<(T, L)>,
    current: T,
    change: impl Fn(T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    segmented_on(
        id,
        items,
        current,
        change,
        None::<fn(Option<T>, &mut Window, &mut App)>,
        cx,
    )
}

pub fn segmented_on<T: Copy + PartialEq + 'static, L: Into<Label>>(
    id: &'static str,
    items: Vec<(T, L)>,
    current: T,
    change: impl Fn(T, &mut Window, &mut App) + 'static,
    hover: Option<impl Fn(Option<T>, &mut Window, &mut App) + 'static>,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let change = Rc::new(change);
    let hover = hover.map(Rc::new);
    let last = items.len().saturating_sub(1);
    let row = div()
        .id(id)
        .flex()
        .flex_none()
        .self_start()
        .overflow_hidden()
        .rounded(px(6.0))
        .border_1()
        .border_color(c.line_strong)
        .when_some(hover.clone(), |row, hover| {
            row.on_hover(move |inside, window, cx| {
                if !*inside {
                    hover(None, window, cx);
                }
            })
        });
    items
        .into_iter()
        .enumerate()
        .fold(row, |row, (index, (value, label))| {
            let chosen = value == current;
            let Label { text, glyph } = label.into();
            let change = change.clone();
            let hover = hover.clone();
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
                .when(glyph.is_some(), |item| {
                    item.flex().items_center().gap(px(6.0))
                })
                .children(glyph.map(|glyph| icon(glyph, TEXT, c.ink)))
                .child(text)
                .when_some(hover, |item, hover| {
                    item.on_hover(move |inside, window, cx| {
                        if *inside {
                            hover(Some(value), window, cx);
                        }
                    })
                })
                .on_click(move |_, window, cx| change(value, window, cx));
            row.child(item)
        })
}
