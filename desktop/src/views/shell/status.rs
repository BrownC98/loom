use gpui::{Div, Entity, ParentElement, Styled, div, px};

use api::graph::NodeState;

use crate::link::LinkState;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::Colors;

use super::zoom::Zoom;

pub const HEIGHT: f32 = 22.0;
const DOT: f32 = 8.0;

fn link_text(link: LinkState) -> Str {
    match link {
        LinkState::Connected => Str::StatusBarLinkConnected,
        LinkState::Reconnecting => Str::StatusBarLinkReconnecting,
        LinkState::Lost => Str::StatusBarLinkLost,
    }
}

pub fn bar(store: &Store, colors: &Colors, lang: Lang, zoom: &Entity<Zoom>) -> Div {
    let (dot, text) = match store.base() {
        None => (colors.err, None),
        Some(_) => {
            let dot = match store.link() {
                LinkState::Connected => colors.ok,
                LinkState::Reconnecting => colors.warn,
                LinkState::Lost => colors.err,
            };
            (dot, Some(link_text(store.link()).text(lang)))
        }
    };
    let counts = store.snapshot().map(|snapshot| {
        let running = snapshot
            .nodes
            .iter()
            .filter(|n| n.state == NodeState::Running)
            .count();
        Str::StatusBarNodeCounts.with(
            lang,
            &[
                ("total", &snapshot.nodes.len().to_string()),
                ("running", &running.to_string()),
            ],
        )
    });
    div()
        .h(px(HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .bg(colors.panel)
        .border_t_1()
        .border_color(colors.line)
        .text_size(px(12.0))
        .text_color(colors.ink_2)
        .child(div().size(px(DOT)).rounded_full().bg(dot))
        .children(text.map(|text| div().child(text)))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_color(colors.ink_3)
                .child(store.status().to_owned()),
        )
        .child(div().flex_1())
        .children(counts.map(|counts| div().child(counts)))
        .child(zoom.clone())
}
