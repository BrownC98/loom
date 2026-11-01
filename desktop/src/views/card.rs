use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Bounds, BoxShadow, Div, ParentElement, Pixels, Rgba, Styled, div, point, px,
};

use api::graph::{CardView, NodeState, NodeView};

use crate::theme::Colors;

pub const HEAD: f32 = 28.0;
const SIDE: f32 = 12.0;
const MIN_TEXT: f32 = 1.0;
const ROUND: f32 = 8.0;
const GROUP_ROUND: f32 = 12.0;
const REPO_ROUND: f32 = 14.0;
const GROUP_LINE: f32 = 1.5;
const REPO_LINE: f32 = 2.0;
const LABEL: f32 = 14.0;
const SMALL: f32 = 12.0;
const HALO: f32 = 3.0;
const FRAME_WASH: f32 = 0.03;
const WAIT_WASH: f32 = 0.35;
const DIM: f32 = 0.4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Plain,
    Lit,
    Dim,
}

pub fn mark(lit: Option<&HashSet<String>>, key: &str) -> Mark {
    match lit {
        None => Mark::Plain,
        Some(keys) if keys.contains(key) => Mark::Lit,
        Some(_) => Mark::Dim,
    }
}

pub fn group_mark(
    lit: Option<&HashSet<String>>,
    path: &str,
    members: impl IntoIterator<Item = String>,
) -> Mark {
    match (lit, mark(lit, path)) {
        (Some(keys), Mark::Dim) if members.into_iter().any(|m| keys.contains(&m)) => Mark::Plain,
        (_, mark) => mark,
    }
}

fn ring(color: Rgba, spread: f32) -> BoxShadow {
    BoxShadow {
        color: color.into(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(spread),
        inset: false,
    }
}

fn wash(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

fn text(size: f32, zoom: f32) -> Pixels {
    px((size * zoom).round().max(MIN_TEXT))
}

pub fn state(state: &NodeState) -> &'static str {
    match state {
        NodeState::Running => "running",
        NodeState::Done => "done",
        NodeState::Exited { .. } => "exited",
        NodeState::SetupFailed { .. } => "setup failed",
    }
}

fn placed(bounds: Bounds<Pixels>) -> Div {
    div()
        .absolute()
        .left(bounds.origin.x)
        .top(bounds.origin.y)
        .w(bounds.size.width)
        .h(bounds.size.height)
}

pub fn frame(
    label: &str,
    bounds: Bounds<Pixels>,
    zoom: f32,
    repo: bool,
    mark: Mark,
    colors: &Colors,
) -> Div {
    let (line, round, line_color) = if repo {
        (REPO_LINE, REPO_ROUND, colors.repo_line)
    } else {
        (GROUP_LINE, GROUP_ROUND, colors.group_line)
    };
    let lit = mark == Mark::Lit;
    placed(bounds)
        .rounded(px(round * zoom))
        .border(px((line * zoom).max(0.5)))
        .border_color(if lit { colors.accent } else { line_color })
        .bg(wash(colors.ink, FRAME_WASH))
        .shadow(if lit {
            vec![ring(colors.accent_wash, HALO * zoom)]
        } else {
            Vec::new()
        })
        .when(mark == Mark::Dim, |d| d.opacity(DIM))
        .px(px(SIDE * zoom))
        .py(px(6.0 * zoom))
        .text_size(text(LABEL, zoom))
        .text_color(colors.ink_3)
        .child(label.to_owned())
}

pub fn card(
    node: &NodeView,
    bounds: Bounds<Pixels>,
    zoom: f32,
    selected: bool,
    mark: Mark,
    body: Option<AnyElement>,
    colors: &Colors,
) -> Div {
    let head = px(HEAD * zoom);
    let simple = node.card_view == Some(CardView::Simple);
    let waiting = !node.waits.is_empty();
    let lit = mark == Mark::Lit;
    let (border, rings) = if selected {
        (colors.accent, vec![ring(colors.accent, 1.0)])
    } else if waiting {
        (
            colors.warn,
            vec![ring(wash(colors.warn, WAIT_WASH), 2.0 * zoom)],
        )
    } else if lit {
        (
            colors.accent,
            vec![
                ring(colors.accent_wash, HALO * zoom),
                ring(colors.accent, 1.0),
            ],
        )
    } else {
        (colors.line_strong, Vec::new())
    };
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
        .when(!simple, |d| {
            d.bg(colors.surface)
                .border_b(px(zoom.max(0.5)))
                .border_color(colors.line)
        })
        .text_size(text(LABEL, zoom))
        .child(node.name.clone())
        .child(
            div()
                .text_size(text(SMALL, zoom))
                .text_color(colors.ink_3)
                .child(format!("#{} {}", node.id, state(&node.state))),
        );
    placed(bounds)
        .overflow_hidden()
        .rounded(px(ROUND * zoom))
        .border(px(zoom.max(0.5)))
        .border_color(border)
        .shadow(if mark == Mark::Dim { Vec::new() } else { rings })
        .when(mark == Mark::Dim, |d| d.opacity(DIM))
        .bg(if simple { colors.surface } else { colors.term })
        .text_color(colors.ink)
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
