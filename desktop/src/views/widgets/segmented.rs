use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, FontWeight, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::btn::tabbable;
use crate::views::icons::{Lucide, icon};
use crate::views::ui::{TEXT, colors, count as count_pill};

pub struct Label {
    text: SharedString,
    glyph: Option<Lucide>,
    count: usize,
}

impl Label {
    pub fn with_icon(text: SharedString, glyph: Lucide) -> Self {
        Self {
            text,
            glyph: Some(glyph),
            count: 0,
        }
    }

    pub fn counted(text: SharedString, count: usize) -> Self {
        Self {
            text,
            glyph: None,
            count,
        }
    }
}

impl From<SharedString> for Label {
    fn from(text: SharedString) -> Self {
        Self {
            text,
            glyph: None,
            count: 0,
        }
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

pub fn segmented_fit<T: Copy + PartialEq + 'static, L: Into<Label>>(
    id: &'static str,
    items: Vec<(T, L)>,
    current: T,
    change: impl Fn(T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    build(
        Width::Fit,
        id,
        items,
        current,
        change,
        None::<fn(Option<T>, &mut Window, &mut App)>,
        cx,
    )
}

pub fn segmented_wide<T: Copy + PartialEq + 'static, L: Into<Label>>(
    id: &'static str,
    items: Vec<(T, L)>,
    current: T,
    change: impl Fn(T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    build(
        Width::Equal,
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
    build(Width::Natural, id, items, current, change, hover, cx)
}

#[derive(Clone, Copy, PartialEq)]
enum Width {
    Natural,
    Equal,
    Fit,
}

fn build<T: Copy + PartialEq + 'static, L: Into<Label>>(
    width: Width,
    id: &'static str,
    items: Vec<(T, L)>,
    current: T,
    change: impl Fn(T, &mut Window, &mut App) + 'static,
    hover: Option<impl Fn(Option<T>, &mut Window, &mut App) + 'static>,
    cx: &App,
) -> Stateful<Div> {
    let wide = width != Width::Natural;
    let c = colors(cx);
    let change = Rc::new(change);
    let hover = hover.map(Rc::new);
    let last = items.len().saturating_sub(1);
    let row = div()
        .id(id)
        .flex()
        .when(wide, |row| row.w_full())
        .when(!wide, |row| row.flex_none().self_start())
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
            let Label { text, glyph, count } = label.into();
            let change = change.clone();
            let hover = hover.clone();
            let item = tabbable(
                div().id(ElementId::NamedInteger(id.into(), index as u64)),
                cx,
            )
            .px(px(14.0))
            .py(px(5.0))
            .text_size(px(TEXT))
            .cursor_pointer()
            .border_color(c.line_strong)
            .when(index != last, |item| item.border_r_1())
            .text_color(if chosen { c.ink } else { c.ink_2 })
            .font_weight(if chosen {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .bg(if chosen {
                c.accent_wash
            } else if wide {
                c.bg
            } else {
                c.surface
            })
            .when(glyph.is_some() || wide, |item| {
                item.flex().items_center().gap(px(6.0))
            })
            .when(wide, |item| {
                item.when(width == Width::Fit, |item| item.flex_auto())
                    .when(width == Width::Equal, |item| item.flex_1())
                    .justify_center()
                    .whitespace_nowrap()
                    .line_height(px(19.5))
            })
            .children(glyph.map(|glyph| icon(glyph, TEXT, c.ink)))
            .child(text)
            .children((count > 0).then(|| count_pill(count, &c)))
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
