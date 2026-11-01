use gpui::{Div, ParentElement, Styled, div, px};

use crate::store::Notice;
use crate::theme::Theme;

const TOP: f32 = 60.0;
const SIDE: f32 = 16.0;
const MAX_WIDTH: f32 = 480.0;

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
        .bg(colors.surface)
        .text_size(px(13.0))
        .text_color(colors.err)
        .child(notice.text.clone())
}
