use std::time::Duration;

use gpui::{Div, ParentElement, Styled, div, px};

use crate::theme::Theme;

const SHOWN_FOR: Duration = Duration::from_secs(6);
const TOP: f32 = 60.0;
const SIDE: f32 = 16.0;
const MAX_WIDTH: f32 = 480.0;

#[derive(Clone)]
pub struct Notice {
    pub text: String,
}

pub const fn shown_for() -> Duration {
    SHOWN_FOR
}

pub fn element(notice: &Notice, theme: &Theme) -> Div {
    let colors = theme.colors;
    div()
        .absolute()
        .top(px(TOP))
        .right(px(SIDE))
        .max_w(px(MAX_WIDTH))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(colors.line_strong)
        .bg(colors.surface)
        .text_size(px(13.0))
        .text_color(colors.err)
        .child(notice.text.clone())
}
