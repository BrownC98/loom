use gpui::{AnyElement, Bounds, Div, ParentElement, Pixels, Styled, div, px, rgb};

use crate::wire::Node;

pub const HEAD: f32 = 28.0;
const SIDE: f32 = 12.0;
const MIN_TEXT: f32 = 1.0;
const ROUND: f32 = 8.0;
const LABEL: f32 = 14.0;
const SMALL: f32 = 12.0;
const EDGE: u32 = 0x4a4d55;
const SELECTED_EDGE: u32 = 0x5b9cf5;
const CARD_BG: u32 = 0x1d2025;
const GROUP_EDGE: u32 = 0x373a41;
const REPO_EDGE: u32 = 0x4b5160;
const TEXT: u32 = 0xe6e8eb;
const MUTED: u32 = 0x8b9099;

fn text(size: f32, zoom: f32) -> Pixels {
    px((size * zoom).round().max(MIN_TEXT))
}

fn placed(bounds: Bounds<Pixels>) -> Div {
    div()
        .absolute()
        .left(bounds.origin.x)
        .top(bounds.origin.y)
        .w(bounds.size.width)
        .h(bounds.size.height)
}

pub fn frame(label: &str, bounds: Bounds<Pixels>, zoom: f32, repo: bool) -> Div {
    placed(bounds)
        .rounded(px(ROUND * zoom))
        .border(px(zoom.max(0.5)))
        .border_color(rgb(if repo { REPO_EDGE } else { GROUP_EDGE }))
        .bg(rgb(if repo { 0x181a1f } else { 0x16181c }))
        .px(px(SIDE * zoom))
        .py(px(6.0 * zoom))
        .text_size(text(LABEL, zoom))
        .text_color(rgb(MUTED))
        .child(label.to_owned())
}

pub fn card(
    node: &Node,
    bounds: Bounds<Pixels>,
    zoom: f32,
    selected: bool,
    body: Option<AnyElement>,
) -> Div {
    let head = px(HEAD * zoom);
    let header = div()
        .absolute()
        .top_0()
        .left_0()
        .w(bounds.size.width)
        .h(head)
        .px(px(SIDE * zoom))
        .flex()
        .items_center()
        .justify_between()
        .text_size(text(LABEL, zoom))
        .child(node.name.clone())
        .child(
            div()
                .text_size(text(SMALL, zoom))
                .text_color(rgb(MUTED))
                .child(format!("#{} {}", node.id, node.state)),
        );
    placed(bounds)
        .overflow_hidden()
        .rounded(px(ROUND * zoom))
        .border(px(zoom.max(0.5)))
        .border_color(rgb(if selected { SELECTED_EDGE } else { EDGE }))
        .bg(rgb(CARD_BG))
        .text_color(rgb(TEXT))
        .child(header)
        .children(body.map(|body| {
            div()
                .absolute()
                .top(head)
                .left_0()
                .w(bounds.size.width)
                .h(bounds.size.height - head)
                .child(body)
        }))
}
