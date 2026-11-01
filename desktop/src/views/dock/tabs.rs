use gpui::prelude::FluentBuilder;
use gpui::{
    Context, Div, FontWeight, InteractiveElement, KeyDownEvent, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, div, px,
};

use super::ui;
use super::{Dock, Tab};
use crate::views::icons::icon;
use crate::views::tooltip::tip;

impl Dock {
    pub(super) fn tab_bar(
        &self,
        tabs: &[Tab],
        shown: Tab,
        waiting: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let c = ui::colors(cx);
        let order = tabs.to_vec();
        let buttons = tabs.iter().map(|&tab| {
            let on = shown == tab;
            let color = if on { c.ink } else { c.ink_2 };
            let label = ui::say(cx, tab.label());
            let hover = c.ink;
            div()
                .id(tab.id())
                .track_focus(&self.tab_focus[tab as usize])
                .tab_stop(on)
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(36.0))
                .px(px(8.0))
                .mb(px(-1.0))
                .min_w_0()
                .border_b_2()
                .border_color(if on {
                    gpui::Hsla::from(c.accent)
                } else {
                    gpui::transparent_black()
                })
                .text_size(px(ui::TEXT))
                .text_color(color)
                .whitespace_nowrap()
                .cursor_pointer()
                .when(on, |tab| tab.flex_none().font_weight(FontWeight::SEMIBOLD))
                .when(!on, |tab| tab.hover(move |style| style.text_color(hover)))
                .tooltip(tip(label.to_owned()))
                .child(icon(tab.glyph(), 14.0, color))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(label),
                )
                .children((tab == Tab::Questions && waiting > 0).then(|| ui::count(waiting, &c)))
                .on_click(cx.listener(move |dock, _, _, cx| dock.choose_tab(tab, cx)))
        });
        div()
            .id("dock-tabs")
            .flex()
            .gap(px(2.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(c.line)
            .on_key_down(cx.listener(move |dock, e: &KeyDownEvent, window, cx| {
                let step: isize = match e.keystroke.key.as_str() {
                    "right" => 1,
                    "left" => -1,
                    _ => return,
                };
                let at = order.iter().position(|&tab| tab == dock.shown).unwrap_or(0);
                let next = order[(at as isize + step).rem_euclid(order.len() as isize) as usize];
                dock.choose_tab(next, cx);
                window.focus(&dock.tab_focus[next as usize], cx);
                cx.stop_propagation();
            }))
            .children(buttons)
    }
}
