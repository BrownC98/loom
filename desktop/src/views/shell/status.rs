mod usage;

use gpui::{App, Div, Entity, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};

use crate::store::Store;
use crate::theme::Colors;

pub const HEIGHT: f32 = 22.0;

#[derive(IntoElement)]
struct Usage(Entity<Store>);

impl RenderOnce for Usage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let store = self.0;
        window.use_keyed_state("status-usage", cx, |_, cx| usage::Usage::new(store, cx))
    }
}

pub fn bar(store: &Entity<Store>, colors: &Colors) -> Div {
    div()
        .h(px(HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .px(px(4.0))
        .bg(colors.panel)
        .border_t_1()
        .border_color(colors.line)
        .text_size(px(12.0))
        .text_color(colors.ink_2)
        .child(Usage(store.clone()))
        .child(div().flex_1())
        .child(usage::version())
}
