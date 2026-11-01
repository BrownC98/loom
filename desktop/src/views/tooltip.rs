use gpui::{AnyView, App, AppContext, IntoElement, ParentElement, Render, Styled, Window, div, px};

use crate::theme::Theme;

const SIZE: f32 = 12.0;

struct Tip(String);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(c.line_strong)
            .bg(c.surface)
            .text_color(c.ink)
            .text_size(px(SIZE))
            .child(self.0.clone())
    }
}

pub fn tip(text: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}
