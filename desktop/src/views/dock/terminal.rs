use api::graph::NodeView;
use gpui::{Context, Div, FontWeight, ParentElement, Styled, Window, div, px};

use super::Dock;
use super::ui::{self, SMALL, TEXT};
use crate::views::card;
use crate::views::terminal;

impl Dock {
    pub(super) fn terminal_panel(
        &mut self,
        node: &NodeView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = ui::colors(cx);
        let id = node.id.0;
        if self.terminal.as_ref().is_none_or(|(held, _)| *held != id) {
            self.terminal = self.store.read(cx).base().map(str::to_owned).map(|base| {
                let view = terminal::open(&self.inputs, &base, id, window, cx);
                view.update(cx, |view, cx| view.focus(window, cx));
                (id, view)
            });
        }
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pt(px(12.0))
            .pb(px(12.0))
            .pl(px(16.0))
            .pr(px(10.0))
            .border_b_1()
            .border_color(c.line)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(node.name.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(format!("#{} {}", node.id, card::state(&node.state))),
            );
        div().size_full().flex().flex_col().child(head).children(
            self.terminal
                .as_ref()
                .map(|(_, view)| div().flex_1().min_h_0().child(view.clone())),
        )
    }
}
