use std::collections::HashSet;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Bounds, BoxShadow, Div, ElementId, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Rgba, Stateful,
    StatefulInteractiveElement, Styled, Window, deferred, div, point, px,
};

use api::graph::waits::{WaitKind, WaitTone};
use api::graph::{CardView, NodeState, NodeView};
use api::repos::PlaceView;
use api::settings::DefaultCardView;

use unicode_width::UnicodeWidthChar;

use crate::canvas::repos::GroupKind;
use crate::fonts;
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::anchor::Anchors;
use crate::views::field::Field;
use crate::views::icons::{Lucide, agent_mark, icon, icon_styled};
use crate::views::tooltip;

pub const HEAD: f32 = 35.0;
pub const FOOT: f32 = 29.0;
const BUTTON: f32 = 26.0;
const BUTTON_ROUND: f32 = 6.0;
const ICON: f32 = 14.0;
const NAME: f32 = 13.0;
const TAG_HEIGHT: f32 = 18.0;
const TAG_TEXT: f32 = 11.0;
const UNREAD_HEIGHT: f32 = 20.0;
const RENAME_WIDTH: f32 = 160.0;
const RENAME_MIN: f32 = 96.0;
const RENAME_HEIGHT: f32 = 22.0;
const MIN_TEXT: f32 = 1.0;
const ROUND: f32 = 8.0;
const GROUP_ROUND: f32 = 12.0;
const REPO_ROUND: f32 = 14.0;
const GROUP_LINE: f32 = 1.5;
const REPO_LINE: f32 = 2.0;
const SMALL: f32 = 12.0;
const HALO: f32 = 3.0;
const FRAME_WASH: f32 = 0.03;
const WAIT_WASH: f32 = 0.35;
const DIM: f32 = 0.4;
const CURRENT_LINE: f32 = 3.0;
const CURRENT_GAP: f32 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Plain,
    Lit,
    Dim,
    Chosen,
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

fn text_px(size: f32, zoom: f32) -> Pixels {
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

fn frame_box(bounds: Bounds<Pixels>, zoom: f32, repo: bool, mark: Mark, colors: &Colors) -> Div {
    let (line, round, line_color) = if repo {
        (REPO_LINE, REPO_ROUND, colors.repo_line)
    } else {
        (GROUP_LINE, GROUP_ROUND, colors.group_line)
    };
    let lit = matches!(mark, Mark::Lit | Mark::Chosen);
    placed(bounds)
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(round * zoom))
        .border(px((line * zoom).max(0.5)))
        .border_color(if lit { colors.accent } else { line_color })
        .bg(wash(colors.ink, FRAME_WASH))
        .shadow(match mark {
            Mark::Lit => vec![ring(colors.accent_wash, HALO * zoom)],
            _ => Vec::new(),
        })
        .when(mark == Mark::Dim, |d| d.opacity(DIM))
        .text_color(colors.ink)
}

pub type Keyed = Rc<dyn Fn(&str, &MouseDownEvent, &mut Window, &mut App)>;

#[derive(Clone)]
pub struct FrameActions {
    pub anchors: Anchors,
    pub arrange: Keyed,
    pub new_node: Keyed,
    pub delete: Keyed,
    pub new_worktree: Keyed,
}

pub struct RepoFrame<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub place: Option<String>,
    pub path: &'a str,
    pub shown_path: &'a str,
    pub has_commits: bool,
}

pub struct ChipView {
    pub path: String,
    pub label: String,
    pub mark: Mark,
}

pub struct GroupFrame<'a> {
    pub path: &'a str,
    pub label: &'a str,
    pub kind: GroupKind,
    pub empty: bool,
    pub chips: Vec<ChipView>,
}

pub fn place_label(place: &PlaceView, lang: Lang) -> Option<String> {
    match place {
        PlaceView::Pc => None,
        PlaceView::Wsl { distro } => Some(Str::PlaceWslDistro.with(lang, &[("distro", distro)])),
        PlaceView::Ssh { host } => Some(Str::PlaceSshHost.with(lang, &[("host", host)])),
    }
}

fn keyed(run: &Keyed, key: &str) -> Press {
    let (run, key) = (run.clone(), key.to_owned());
    Rc::new(move |e, window, cx| run(&key, e, window, cx))
}

#[allow(clippy::too_many_arguments)]
fn text_button(
    id: ElementId,
    glyph: Option<Lucide>,
    text: String,
    disabled: bool,
    tip: Option<String>,
    anchor: Option<(&Anchors, String)>,
    press: Press,
    zoom: f32,
    colors: &Colors,
) -> Stateful<Div> {
    let (hover, ink_2, ink) = (colors.surface_hover, colors.ink_2, colors.ink);
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0 * zoom))
        .h(px(BUTTON * zoom))
        .px(px(8.0 * zoom))
        .rounded(px(BUTTON_ROUND * zoom))
        .text_size(text_px(SMALL, zoom))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink_2)
        .whitespace_nowrap()
        .when(disabled, |d| d.opacity(0.6))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover).text_color(ink))
        })
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            if !disabled {
                press(e, window, cx);
            }
            cx.stop_propagation();
        })
        .when_some(tip, |d, tip| d.tooltip(tooltip::tip(tip)))
        .children(anchor.map(|(anchors, key)| anchors.mark(key)))
        .children(glyph.map(|g| icon(g, (ICON * zoom).max(1.0), ink_2)))
        .child(text)
}

fn band(height: f32, zoom: f32, colors: &Colors) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0 * zoom))
        .w_full()
        .h(px(height * zoom))
        .border_b(px(zoom.max(0.5)))
        .border_color(colors.line)
}

pub fn repo_frame(
    info: &RepoFrame,
    acts: &FrameActions,
    lang: Lang,
    bounds: Bounds<Pixels>,
    zoom: f32,
    mark: Mark,
    colors: &Colors,
) -> Div {
    let key = info.key;
    let arrange = ghost(
        ElementId::Name(format!("arrange:{key}").into()),
        Some((&acts.anchors, format!("arrange:{key}"))),
        Lucide::Workflow,
        Str::GraphArrangeRepo.text(lang),
        &keyed(&acts.arrange, key),
        zoom,
        colors,
    );
    let tip = if info.has_commits {
        None
    } else {
        Some(format!(
            "{}: {}",
            Str::RepoNewWorktree.text(lang),
            Str::RepoNoCommit.text(lang)
        ))
    };
    let new_worktree = text_button(
        ElementId::Name(format!("wt:{key}").into()),
        Some(Lucide::GitBranchPlus),
        Str::RepoNewWorktree.text(lang).to_owned(),
        !info.has_commits,
        tip,
        Some((&acts.anchors, format!("wt:{key}"))),
        keyed(&acts.new_worktree, key),
        zoom,
        colors,
    );
    let head = band(46.0, zoom, colors)
        .px(px(16.0 * zoom))
        .child(
            div()
                .flex_none()
                .text_size(text_px(15.0, zoom))
                .font_weight(FontWeight::SEMIBOLD)
                .child(info.name.to_owned()),
        )
        .children(info.place.clone().map(|text| {
            div()
                .flex_none()
                .px(px(6.0 * zoom))
                .h(px(18.0 * zoom))
                .flex()
                .items_center()
                .rounded(px(9.0 * zoom))
                .border(px(zoom.max(0.5)))
                .border_color(colors.line_strong)
                .text_size(text_px(11.0, zoom))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.ink_2)
                .child(text)
        }))
        .child(
            div()
                .id(ElementId::Name(format!("repo-path:{key}").into()))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(text_px(SMALL, zoom))
                .text_color(colors.ink_3)
                .tooltip(tooltip::tip(info.path.to_owned()))
                .child(info.shown_path.to_owned()),
        )
        .child(div().flex_1())
        .child(arrange)
        .child(new_worktree);
    frame_box(bounds, zoom, true, mark, colors).child(head)
}

pub fn group_frame(
    info: &GroupFrame,
    acts: &FrameActions,
    lang: Lang,
    bounds: Bounds<Pixels>,
    zoom: f32,
    mark: Mark,
    colors: &Colors,
) -> Div {
    let path = info.path;
    let kind = match info.kind {
        GroupKind::Original => Some(Str::WorktreeOriginal),
        GroupKind::Unborn => Some(Str::WorktreeUnborn),
        GroupKind::Linked => None,
    };
    let mut head = band(43.0, zoom, colors)
        .pl(px(14.0 * zoom))
        .pr(px(10.0 * zoom))
        .child(
            div()
                .id(ElementId::Name(format!("group-branch:{path}").into()))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(text_px(NAME, zoom))
                .font_weight(FontWeight::SEMIBOLD)
                .tooltip(tooltip::tip(info.label.to_owned()))
                .child(info.label.to_owned()),
        )
        .children(kind.map(|k| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(text_px(SMALL, zoom))
                .text_color(colors.ink_3)
                .child(k.text(lang))
        }))
        .child(div().flex_1())
        .child(ghost(
            ElementId::Name(format!("arrange:{path}").into()),
            None,
            Lucide::Workflow,
            Str::GraphArrangeGroup.text(lang),
            &keyed(&acts.arrange, path),
            zoom,
            colors,
        ))
        .child(text_button(
            ElementId::Name(format!("new:{path}").into()),
            Some(Lucide::Plus),
            Str::GraphNewNode.text(lang).to_owned(),
            false,
            None,
            Some((&acts.anchors, format!("new:{path}"))),
            keyed(&acts.new_node, path),
            zoom,
            colors,
        ));
    if info.kind == GroupKind::Linked {
        head = head.child(text_button(
            ElementId::Name(format!("del:{path}").into()),
            None,
            Str::DeleteWorktreeAction.text(lang).to_owned(),
            false,
            None,
            Some((&acts.anchors, format!("del:{path}"))),
            keyed(&acts.delete, path),
            zoom,
            colors,
        ));
    }
    let chips = (!info.chips.is_empty()).then(|| {
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_x(px(12.0 * zoom))
            .gap_y(px(6.0 * zoom))
            .px(px(14.0 * zoom))
            .py(px(7.0 * zoom))
            .w_full()
            .border_b(px(zoom.max(0.5)))
            .border_color(colors.line)
            .child(
                div()
                    .mr(px(2.0 * zoom))
                    .text_size(text_px(SMALL, zoom))
                    .text_color(colors.ink_3)
                    .child(Str::GraphChips.text(lang)),
            )
            .children(info.chips.iter().map(|chip| {
                let lit = chip.mark == Mark::Lit;
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0 * zoom))
                    .h(px(28.0 * zoom))
                    .pl(px(8.0 * zoom))
                    .pr(px(3.0 * zoom))
                    .rounded(px(8.0 * zoom))
                    .border(px(zoom.max(0.5)))
                    .border_color(if lit {
                        colors.accent
                    } else {
                        colors.group_line
                    })
                    .bg(colors.surface)
                    .when(lit, |d| d.shadow(vec![ring(colors.accent, 1.0)]))
                    .when(chip.mark == Mark::Dim, |d| d.opacity(DIM))
                    .child(
                        div()
                            .id(ElementId::Name(format!("chip-branch:{}", chip.path).into()))
                            .mr(px(4.0 * zoom))
                            .max_w(px(240.0 * zoom))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(text_px(SMALL, zoom))
                            .font_weight(FontWeight::SEMIBOLD)
                            .tooltip(tooltip::tip(chip.path.clone()))
                            .child(chip.label.clone()),
                    )
                    .child(ghost_sized(
                        ElementId::Name(format!("new:{}", chip.path).into()),
                        Some((&acts.anchors, format!("new:{}", chip.path))),
                        Lucide::Plus,
                        Str::GraphNewNode.text(lang),
                        &keyed(&acts.new_node, &chip.path),
                        24.0,
                        zoom,
                        colors,
                    ))
                    .child(ghost_sized(
                        ElementId::Name(format!("del:{}", chip.path).into()),
                        Some((&acts.anchors, format!("del:{}", chip.path))),
                        Lucide::Trash2,
                        Str::DeleteWorktreeAction.text(lang),
                        &keyed(&acts.delete, &chip.path),
                        24.0,
                        zoom,
                        colors,
                    ))
            }))
    });
    frame_box(bounds, zoom, false, mark, colors)
        .child(head)
        .children(chips)
        .children(info.empty.then(|| {
            div()
                .px(px(14.0 * zoom))
                .py(px(16.0 * zoom))
                .text_size(text_px(NAME, zoom))
                .text_color(colors.ink_3)
                .child(Str::GraphEmpty.text(lang))
        }))
}

pub const SIMPLE_WIDTH: f64 = 260.0;
const REPLY_WIDTH: f64 = SIMPLE_WIDTH - 22.0;
const REPLY_LINE: f64 = 18.0;
const REPLY_LINES: usize = 2;
const GLYPH: f64 = 6.2;

fn reply_lines(text: &str) -> usize {
    let width = |word: &str| {
        word.chars()
            .map(|c| c.width().unwrap_or(1) as f64 * GLYPH)
            .sum::<f64>()
    };
    let mut lines = 1usize;
    let mut used = 0.0;
    for word in text.split_whitespace() {
        let w = width(word);
        if used == 0.0 {
            used = w;
        } else if used + GLYPH + w <= REPLY_WIDTH {
            used += GLYPH + w;
        } else {
            lines += 1;
            used = w;
        }
        if used > REPLY_WIDTH {
            lines += (used / REPLY_WIDTH).ceil() as usize - 1;
            used %= REPLY_WIDTH;
        }
    }
    lines
}

pub fn simple_height(node: &NodeView) -> f64 {
    let mut height = 2.0 + 16.0 + f64::from(BUTTON);
    if !model_text(node).is_empty() {
        height += 4.0 + 16.0;
    }
    if let Some(reply) = &node.reply {
        height += 4.0 + REPLY_LINE * reply_lines(reply).min(REPLY_LINES) as f64;
    }
    height
}

pub fn base_view(node: &NodeView, setting: DefaultCardView) -> CardView {
    match setting {
        DefaultCardView::Standard => CardView::Standard,
        DefaultCardView::Simple => CardView::Simple,
        DefaultCardView::RootTerminal if node.parent.is_none() => CardView::Standard,
        DefaultCardView::RootTerminal => CardView::Simple,
    }
}

pub fn view_of(node: &NodeView, setting: DefaultCardView) -> CardView {
    node.card_view.unwrap_or_else(|| base_view(node, setting))
}

pub type Press = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

#[derive(Clone)]
pub struct Actions {
    pub anchors: Anchors,
    pub steer: Press,
    pub bookmark: Press,
    pub expand: Press,
    pub menu: Press,
    pub rename: Press,
}

#[derive(Clone, Copy)]
pub struct Look {
    pub simple: bool,
    pub selected: bool,
    pub mark: Mark,
}

pub struct Face<'a> {
    pub lang: Lang,
    pub node: &'a NodeView,
    pub parent: Option<&'a str>,
    pub rename: Option<(&'a Entity<Field>, Option<&'a str>)>,
    pub actions: &'a Actions,
    pub collapse: bool,
    pub width: f32,
}

pub fn model_text(node: &NodeView) -> String {
    [node.model.as_deref(), node.effort.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn state_text(state: &NodeState, lang: Lang) -> String {
    match state {
        NodeState::Running => Str::NodeStateRunning.text(lang).to_owned(),
        NodeState::Done => Str::NodeStateDone.text(lang).to_owned(),
        NodeState::Exited { code } => Str::NodeStateExited.with(
            lang,
            &[(
                "code",
                &code.map_or_else(|| "?".to_owned(), |c| c.to_string()),
            )],
        ),
        NodeState::SetupFailed { .. } => Str::NodeStateSetupFailed.text(lang).to_owned(),
    }
}

pub fn foot_height(node: &NodeView) -> f32 {
    match node.state {
        NodeState::Running | NodeState::Done => 0.0,
        NodeState::Exited { .. } | NodeState::SetupFailed { .. } => FOOT,
    }
}

pub fn body_height(card_height: f32, node: &NodeView) -> f32 {
    card_height - HEAD - foot_height(node) - 2.0
}

pub fn foot_text(node: &NodeView, parent: Option<&str>, lang: Lang) -> Option<String> {
    match node.state {
        NodeState::Exited { code: None } => {
            Some(Str::NodeFootDaemonRestarted.text(lang).to_owned())
        }
        NodeState::Exited { code: Some(code) } => {
            let code = code.to_string();
            Some(match parent {
                Some(parent) => {
                    Str::NodeFootExitedSent.with(lang, &[("code", &code), ("parent", parent)])
                }
                None => Str::NodeFootExited.with(lang, &[("code", &code)]),
            })
        }
        NodeState::SetupFailed { code } => {
            Some(Str::NodeFootSetupFailed.with(lang, &[("code", &code.to_string())]))
        }
        NodeState::Running | NodeState::Done => None,
    }
}

fn wait_kind(kind: &WaitKind) -> Str {
    match kind {
        WaitKind::Question => Str::WaitQuestion,
        WaitKind::Approval => Str::WaitApproval,
        WaitKind::Trust => Str::WaitTrust,
        WaitKind::Auth => Str::WaitAuth,
        WaitKind::Input => Str::WaitInput,
        WaitKind::Done => Str::WaitDone,
    }
}

fn button(id: impl Into<ElementId>, size: f32, zoom: f32) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px((size * zoom).max(1.0)))
        .rounded(px(BUTTON_ROUND * zoom))
}

#[allow(clippy::too_many_arguments)]
fn ghost_sized(
    id: ElementId,
    anchor: Option<(&Anchors, String)>,
    glyph: Lucide,
    tip: &str,
    press: &Press,
    size: f32,
    zoom: f32,
    colors: &Colors,
) -> Stateful<Div> {
    let (hover, ink_2, ink) = (colors.surface_hover, colors.ink_2, colors.ink);
    let press = press.clone();
    button(id, size, zoom)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(ink))
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            press(e, window, cx);
            cx.stop_propagation();
        })
        .tooltip(tooltip::tip(tip.to_owned()))
        .children(anchor.map(|(anchors, key)| anchors.mark(key)))
        .child(icon(glyph, (ICON * zoom).max(1.0), ink_2))
}

fn ghost(
    id: ElementId,
    anchor: Option<(&Anchors, String)>,
    glyph: Lucide,
    tip: &str,
    press: &Press,
    zoom: f32,
    colors: &Colors,
) -> Stateful<Div> {
    ghost_sized(id, anchor, glyph, tip, press, BUTTON, zoom, colors)
}

fn bookmark_button(face: &Face, zoom: f32, colors: &Colors) -> Stateful<Div> {
    let on = face.node.bookmark.is_some();
    let tip = if on {
        Str::BookmarkRemove
    } else {
        Str::BookmarkAdd
    };
    let color = if on { colors.ink } else { colors.ink_2 };
    let (hover, press) = (colors.surface_hover, face.actions.bookmark.clone());
    button(("bookmark", face.node.id.0 as usize), BUTTON, zoom)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            press(e, window, cx);
            cx.stop_propagation();
        })
        .tooltip(tooltip::tip(tip.text(face.lang).to_owned()))
        .child(icon_styled(
            Lucide::Bookmark,
            (ICON * zoom).max(1.0),
            color,
            2.0,
            on,
        ))
}

fn tag(text: String, fill: Rgba, ink: Rgba, zoom: f32) -> Div {
    div()
        .flex_none()
        .h(px(TAG_HEIGHT * zoom))
        .px(px(7.0 * zoom))
        .flex()
        .items_center()
        .rounded(px(9.0 * zoom))
        .border(px((zoom).max(0.5)))
        .border_color(fill)
        .bg(fill)
        .text_size(text_px(TAG_TEXT, zoom))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .whitespace_nowrap()
        .child(text)
}

fn badges(face: &Face, zoom: f32, colors: &Colors) -> Vec<AnyElement> {
    let (node, lang) = (face.node, face.lang);
    let mut out: Vec<AnyElement> = Vec::new();
    if node.unread > 0 {
        let press = face.actions.steer.clone();
        let hover_fill = colors.accent;
        out.push(
            div()
                .id(("unread", node.id.0 as usize))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.0 * zoom))
                .h(px(UNREAD_HEIGHT * zoom))
                .px(px(7.0 * zoom))
                .rounded(px(UNREAD_HEIGHT / 2.0 * zoom))
                .bg(colors.accent)
                .text_color(colors.accent_ink)
                .text_size(text_px(SMALL, zoom))
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(move |s| s.bg(hover_fill).opacity(0.9))
                .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                    press(e, window, cx);
                    cx.stop_propagation();
                })
                .tooltip(tooltip::tip(Str::NodeSteerQueued.text(lang).to_owned()))
                .child(icon(
                    Lucide::Mail,
                    (13.0 * zoom).max(1.0),
                    colors.accent_ink,
                ))
                .child(Str::NodeUnreadShort.with(lang, &[("n", &node.unread.to_string())]))
                .into_any_element(),
        );
    }
    let asks: Vec<_> = node
        .waits
        .iter()
        .filter(|w| matches!(w.tone, WaitTone::Ask))
        .collect();
    let dones = node.waits.len() - asks.len();
    if let Some(oldest) = asks.iter().min_by_key(|w| w.wait.since) {
        out.push(
            tag(
                format!("{} {}", wait_kind(&oldest.wait.kind).text(lang), asks.len()),
                colors.ask,
                colors.ask_ink,
                zoom,
            )
            .into_any_element(),
        );
    }
    if dones > 0 {
        out.push(
            tag(
                Str::TreeDone.with(lang, &[("n", &dones.to_string())]),
                colors.ok,
                colors.surface,
                zoom,
            )
            .into_any_element(),
        );
    }
    if matches!(
        node.state,
        NodeState::Exited { .. } | NodeState::SetupFailed { .. }
    ) {
        out.push(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.0 * zoom))
                .text_size(text_px(SMALL, zoom))
                .font_family(fonts::TERMINAL_FAMILY)
                .text_color(colors.err)
                .whitespace_nowrap()
                .child(
                    div()
                        .size(px((7.0 * zoom).max(1.0)))
                        .rounded_full()
                        .bg(colors.err),
                )
                .child(state_text(&node.state, lang))
                .into_any_element(),
        );
    }
    out
}

fn name_slot(face: &Face, zoom: f32, colors: &Colors) -> AnyElement {
    let node = face.node;
    if let Some((field, error)) = face.rename {
        let hint = error.map_or_else(|| Str::RenameHint.text(face.lang).to_owned(), str::to_owned);
        let tip = deferred(
            div()
                .absolute()
                .left_0()
                .top(px((RENAME_HEIGHT + 4.0) * zoom))
                .max_w(px(260.0 * zoom))
                .px(px(8.0 * zoom))
                .py(px(4.0 * zoom))
                .rounded(px(6.0 * zoom))
                .border(px(zoom.max(0.5)))
                .border_color(colors.line_strong)
                .bg(colors.surface)
                .shadow_md()
                .text_size(text_px(SMALL, zoom))
                .when(error.is_none(), |d| d.whitespace_nowrap())
                .text_color(if error.is_some() {
                    colors.err
                } else {
                    colors.ink_2
                })
                .child(hint),
        )
        .with_priority(3);
        return div()
            .relative()
            .flex_none()
            .w(px((RENAME_WIDTH * zoom)
                .min(face.width * 0.45 * zoom)
                .max(RENAME_MIN * zoom)))
            .h(px(RENAME_HEIGHT * zoom))
            .px(px(6.0 * zoom))
            .flex()
            .items_center()
            .rounded(px(4.0 * zoom))
            .border(px(zoom.max(0.5)))
            .border_color(if error.is_some() {
                colors.err
            } else {
                colors.accent
            })
            .bg(colors.bg)
            .shadow(vec![ring(
                if error.is_some() {
                    wash(colors.err, 0.25)
                } else {
                    colors.accent_wash
                },
                2.0 * zoom,
            )])
            .text_size(text_px(NAME, zoom))
            .text_color(colors.ink)
            .child(field.clone())
            .child(tip)
            .into_any_element();
    }
    let press = face.actions.rename.clone();
    div()
        .id(("name", node.id.0 as usize))
        .min_w_0()
        .max_w(px(face.width * 0.45 * zoom))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(text_px(NAME, zoom))
        .font_weight(FontWeight::SEMIBOLD)
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            if e.click_count == 2 {
                press(e, window, cx);
                cx.stop_propagation();
            }
        })
        .tooltip(tooltip::tip(Str::RenameTitle.text(face.lang).to_owned()))
        .child(node.name.clone())
        .into_any_element()
}

fn mark_of(face: &Face, zoom: f32, colors: &Colors) -> AnyElement {
    let label = face.node.agent.as_ref().map_or_else(
        || Str::NodeTerminal.text(face.lang).to_owned(),
        |a| a.0.clone(),
    );
    agent_mark(
        face.node.agent.as_ref().map(|a| a.0.as_str()),
        &label,
        (ICON * zoom).max(1.0),
        colors.ink_2,
    )
}

fn actions(face: &Face, expand: bool, zoom: f32, colors: &Colors) -> Vec<AnyElement> {
    let (node, lang) = (face.node.id.0, face.lang);
    let mut out = vec![bookmark_button(face, zoom, colors).into_any_element()];
    if expand {
        let (glyph, tip) = if face.collapse {
            (Lucide::Minimize2, Str::NodeCollapse)
        } else {
            (Lucide::Maximize2, Str::NodeExpandTitle)
        };
        out.push(
            ghost(
                ElementId::NamedInteger("expand".into(), node as u64),
                None,
                glyph,
                tip.text(lang),
                &face.actions.expand,
                zoom,
                colors,
            )
            .into_any_element(),
        );
    }
    out.push(
        ghost(
            ElementId::NamedInteger("menu".into(), node as u64),
            Some((&face.actions.anchors, format!("menu:{node}"))),
            Lucide::Ellipsis,
            Str::NodeMenu.text(lang),
            &face.actions.menu,
            zoom,
            colors,
        )
        .into_any_element(),
    );
    out
}

pub fn head(face: &Face, zoom: f32, colors: &Colors) -> Div {
    let model = model_text(face.node);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0 * zoom))
        .w_full()
        .h(px(HEAD * zoom))
        .pl(px(10.0 * zoom))
        .pr(px(6.0 * zoom))
        .bg(colors.surface)
        .border_b(px(zoom.max(0.5)))
        .border_color(colors.line)
        .text_size(text_px(NAME, zoom))
        .text_color(colors.ink)
        .child(name_slot(face, zoom, colors))
        .child(mark_of(face, zoom, colors))
        .when(!model.is_empty(), |d| {
            d.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(text_px(SMALL, zoom))
                    .font_family(fonts::TERMINAL_FAMILY)
                    .text_color(colors.ink_3)
                    .child(model),
            )
        })
        .child(div().flex_1())
        .children(badges(face, zoom, colors))
        .children(actions(face, true, zoom, colors))
}

fn summary(face: &Face, zoom: f32, colors: &Colors) -> Div {
    let model = model_text(face.node);
    let row = div()
        .flex()
        .items_center()
        .gap(px(8.0 * zoom))
        .min_h(px(24.0 * zoom))
        .child(mark_of(face, zoom, colors))
        .child(name_slot(face, zoom, colors))
        .child(div().flex_1())
        .children(badges(face, zoom, colors))
        .children(actions(face, false, zoom, colors));
    div()
        .flex()
        .flex_col()
        .gap(px(4.0 * zoom))
        .min_w_0()
        .child(row)
        .when(!model.is_empty(), |d| {
            d.child(
                div()
                    .text_size(text_px(SMALL, zoom))
                    .font_family(fonts::TERMINAL_FAMILY)
                    .text_color(colors.ink_3)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(model),
            )
        })
        .children(face.node.reply.clone().map(|reply| {
            div()
                .id(("reply", face.node.id.0 as usize))
                .text_size(text_px(SMALL, zoom))
                .text_color(colors.ink_2)
                .overflow_hidden()
                .line_clamp(2)
                .tooltip(tooltip::tip(reply.clone()))
                .child(reply)
        }))
}

pub fn foot(face: &Face, selected: bool, zoom: f32, colors: &Colors) -> Option<Div> {
    let text = foot_text(face.node, face.parent, face.lang)?;
    Some(
        div()
            .w_full()
            .h(px(FOOT * zoom))
            .px(px(10.0 * zoom))
            .flex()
            .flex_none()
            .items_center()
            .border_t(px(zoom.max(0.5)))
            .border_color(colors.line)
            .text_size(text_px(SMALL, zoom))
            .text_color(if selected {
                colors.accent
            } else {
                colors.ink_3
            })
            .child(text),
    )
}

pub fn card(
    face: &Face,
    look: Look,
    bounds: Bounds<Pixels>,
    zoom: f32,
    body: Option<AnyElement>,
    colors: &Colors,
) -> Div {
    let Look {
        simple,
        selected,
        mark,
    } = look;
    let node = face.node;
    let head_h = px(HEAD * zoom);
    let foot = (!simple)
        .then(|| foot(face, selected, zoom, colors))
        .flatten();
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
    let shell = placed(bounds)
        .overflow_hidden()
        .rounded(px(ROUND * zoom))
        .border(px(zoom.max(0.5)))
        .border_color(border)
        .shadow(if mark == Mark::Dim { Vec::new() } else { rings })
        .when(mark == Mark::Dim, |d| d.opacity(DIM))
        .bg(if simple { colors.surface } else { colors.term })
        .text_color(colors.ink);
    if simple {
        return shell
            .h_auto()
            .pt(px(8.0 * zoom))
            .pb(px(8.0 * zoom))
            .pl(px(12.0 * zoom))
            .pr(px(8.0 * zoom))
            .child(summary(face, zoom, colors));
    }
    shell
        .child(head(face, zoom, colors).absolute().top_0().left_0())
        .children(body.map(|body| {
            div()
                .absolute()
                .top(head_h)
                .left_0()
                .w_full()
                .h(px(
                    body_height(f32::from(bounds.size.height) / zoom, node) * zoom
                ))
                .child(body)
        }))
        .children(foot.map(|row| row.absolute().bottom_0().left_0()))
}

pub fn current(bounds: Bounds<Pixels>, zoom: f32, group: bool, colors: &Colors) -> Div {
    let grow = px(CURRENT_GAP * zoom);
    let round = if group { GROUP_ROUND } else { ROUND };
    placed(Bounds::new(
        bounds.origin - point(grow, grow),
        bounds.size + gpui::size(grow * 2.0, grow * 2.0),
    ))
    .rounded(px((round + CURRENT_GAP) * zoom))
    .border(px((CURRENT_LINE * zoom).max(1.0)))
    .border_color(colors.warn)
}
