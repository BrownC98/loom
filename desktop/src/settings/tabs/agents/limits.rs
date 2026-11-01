use gpui::{App, AppContext, Entity, ParentElement, Styled, Subscription, Window, div, px};

use super::parts::{hint, labelled};
use crate::settings::widgets::{Number, NumberEvent, field_reset};
use crate::settings::{Ctx, Section};
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, colors, say};

const DEPTH: std::ops::RangeInclusive<i64> = 0..=9;
const CHILDREN: std::ops::RangeInclusive<i64> = 1..=99;

#[derive(Default)]
pub struct State {
    depth: Option<Entity<Number>>,
    children: Option<Entity<Number>>,
    subs: Vec<Subscription>,
}

pub fn section(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) -> Section {
    let c = colors(cx);
    let limits = ctx.view.tree_limits;
    let depth = state
        .depth
        .get_or_insert_with(|| {
            let writer = ctx.writer.clone();
            let number = cx.new(|cx| Number::new(DEPTH, true, false, window, cx));
            state
                .subs
                .push(cx.subscribe(&number, move |_, event: &NumberEvent, cx| {
                    let NumberEvent::Commit(value) = *event;
                    writer.edit(cx, move |settings| {
                        settings.tree_limits.max_depth = value.map(|n| n as u32)
                    });
                }));
            number
        })
        .clone();
    let children = state
        .children
        .get_or_insert_with(|| {
            let writer = ctx.writer.clone();
            let number = cx.new(|cx| Number::new(CHILDREN, true, false, window, cx));
            state
                .subs
                .push(cx.subscribe(&number, move |_, event: &NumberEvent, cx| {
                    let NumberEvent::Commit(value) = *event;
                    writer.edit(cx, move |settings| {
                        settings.tree_limits.max_children = value.map(|n| n as u32)
                    });
                }));
            number
        })
        .clone();
    depth.update(cx, |number, cx| {
        number.show(limits.max_depth.map(i64::from), window, cx)
    });
    children.update(cx, |number, cx| {
        number.show(limits.max_children.map(i64::from), window, cx)
    });

    let mut section = Section::new(Some(say(cx, Str::SettingsTreeLimits)), ctx);
    let none = say(cx, Str::SettingsNoLimit);
    let mut field = |label: Str, number: Entity<Number>, empty: bool| {
        let label = section.text(say(cx, label), cx);
        let none = section.text(none, cx);
        labelled(
            label,
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(number)
                .children(
                    empty.then(|| div().text_size(px(SMALL)).text_color(c.ink_3).child(none)),
                ),
            cx,
        )
    };
    let depth_field = field(Str::SettingsMaxDepth, depth, limits.max_depth.is_none());
    let children_field = field(
        Str::SettingsMaxChildren,
        children,
        limits.max_children.is_none(),
    );
    let note = section.text(say(cx, Str::SettingsTreeLimitsHint), cx);
    section
        .beside(field_reset("agents-limits-reset", &["treeLimits"], ctx, cx))
        .child(
            div()
                .flex()
                .gap(px(16.0))
                .child(depth_field)
                .child(children_field),
        )
        .child(hint(note, cx))
}
