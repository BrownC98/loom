use gpui::{Div, ParentElement, Styled, div, px};

use crate::store::{Level, Notice};
use crate::theme::Theme;

const TOP: f32 = 60.0;
const SIDE: f32 = 16.0;
const MAX_WIDTH: f32 = 480.0;

pub fn element(notice: &Notice, theme: &Theme) -> Div {
    let colors = theme.colors;
    let tone = match notice.level {
        Level::Info => colors.ink,
        Level::Ok => colors.ok,
        Level::Warn => colors.warn,
        Level::Error => colors.err,
    };
    div()
        .absolute()
        .top(px(TOP))
        .right(px(SIDE))
        .max_w(px(MAX_WIDTH))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(tone)
        .bg(colors.surface)
        .text_size(px(13.0))
        .text_color(tone)
        .child(notice.text.clone())
}
