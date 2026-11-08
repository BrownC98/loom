use api::graph::waits::WaitTone;
use gpui::{
    Div, FontWeight, InteractiveElement, IntoElement, ParentElement, Rgba, Stateful, Styled, Svg,
    div, px, svg,
};

use super::ui::{self, SMALL};
use crate::fonts;
use crate::strings::Str;
use crate::theme::{Colors, mix};
use crate::views::icons::{Lucide, icon};

pub(crate) fn tone_of(tone: WaitTone, c: &Colors) -> Rgba {
    match tone {
        WaitTone::Ask => c.warn,
        WaitTone::Done => c.ok,
    }
}

pub(crate) fn tone_ink(tone: WaitTone, c: &Colors) -> Rgba {
    match tone {
        WaitTone::Ask => c.warn_ink,
        WaitTone::Done => c.surface,
    }
}

pub(crate) fn mark(tone: WaitTone, z: f32, c: &Colors) -> Svg {
    let glyph = match tone {
        WaitTone::Ask => Lucide::CircleQuestionMark,
        WaitTone::Done => Lucide::CircleCheck,
    };
    icon(glyph, 15.0 * z, tone_of(tone, c))
}

pub(crate) fn qid(text: String, tone: WaitTone, height: f32, z: f32, c: &Colors) -> Div {
    let color = tone_of(tone, c);
    let mut edge = color;
    edge.a = 0.55;
    div()
        .flex_none()
        .flex()
        .items_center()
        .h(px(height * z))
        .px(px(6.0 * z))
        .rounded(px(4.0 * z))
        .border_1()
        .border_color(edge)
        .bg(mix(color, 0.1, c.surface))
        .text_color(c.ink)
        .text_size(px(11.0 * z))
        .font_weight(FontWeight::SEMIBOLD)
        .font_family(fonts::TERMINAL_FAMILY)
        .whitespace_nowrap()
        .child(text)
}

pub(crate) fn ghost_close(id: &'static str, key: usize, z: f32, c: &Colors) -> Stateful<Div> {
    let (hover, ink) = (c.line, c.ink);
    let group = format!("{id}-{key}");
    div()
        .id((id, key))
        .group(group.clone())
        .flex_none()
        .size(px(22.0 * z))
        .rounded(px(5.0 * z))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(
            svg()
                .path(Lucide::X.path())
                .size(px(14.0 * z))
                .flex_none()
                .text_color(c.ink_3)
                .group_hover(group, move |style| style.text_color(ink)),
        )
}

pub(crate) fn clip(label: &str, z: f32, c: &Colors) -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .text_size(px(SMALL * z))
        .text_color(c.ink_3)
        .child(label.to_owned())
}

pub(crate) fn published(since: i64, z: f32, cx: &gpui::App) -> Div {
    let c = ui::colors(cx);
    let time = ui::clock(since, "%Y/%m/%d %H:%M:%S");
    div()
        .min_w_0()
        .ml_auto()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(SMALL * z))
        .text_color(c.ink_3)
        .child(ui::say_with(cx, Str::InboxPublished, &[("time", &time)]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    Plain,
    Recommended,
    Chosen,
    Passed,
}

pub(crate) struct Choice<'a> {
    pub key: String,
    pub hint: Option<String>,
    pub label: &'a str,
    pub recommended: bool,
    pub look: Look,
    pub tone: WaitTone,
}

pub(crate) fn choice_face<E: Styled + ParentElement>(
    base: E,
    choice: Choice,
    z: f32,
    cx: &gpui::App,
) -> E {
    let Choice {
        key,
        hint,
        label,
        recommended,
        look,
        tone,
    } = choice;
    let c = ui::colors(cx);
    let color = tone_of(tone, &c);
    let chosen = look == Look::Chosen;
    let mut face = base
        .flex()
        .items_baseline()
        .gap(px(6.0 * z))
        .min_h(px(26.0 * z))
        .px(px(if chosen { 9.0 } else { 10.0 } * z))
        .py(px(if chosen { 2.0 } else { 3.0 } * z))
        .rounded(px(6.0 * z))
        .border_color(if chosen {
            c.ink
        } else if recommended {
            color
        } else {
            c.line_strong
        })
        .bg(if recommended {
            mix(color, 0.1, c.surface)
        } else {
            c.surface
        })
        .text_color(c.ink)
        .text_size(px(SMALL * z))
        .font_weight(if chosen {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        });
    face = if chosen {
        face.border(px(2.0 * z))
    } else {
        face.border(px(z))
    };
    let mut label_box = div().flex().items_baseline().child(label.to_owned());
    if recommended {
        label_box = label_box.child(
            div()
                .ml(px(6.0 * z))
                .text_size(px(11.0 * z))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .whitespace_nowrap()
                .child(ui::say(cx, Str::InboxRecommended)),
        );
    }
    face.children(hint.map(|text| {
        div()
            .text_size(px(10.0 * z))
            .font_family(fonts::TERMINAL_FAMILY)
            .text_color(c.ink_3)
            .child(text)
    }))
    .child(
        div()
            .text_size(px(11.0 * z))
            .font_weight(FontWeight::SEMIBOLD)
            .font_family(fonts::TERMINAL_FAMILY)
            .text_color(c.ink_3)
            .child(key),
    )
    .child(label_box)
}

pub(crate) fn chosen_mark(z: f32, cx: &gpui::App) -> Div {
    let c = ui::colors(cx);
    div()
        .flex()
        .items_center()
        .gap(px(3.0 * z))
        .ml(px(4.0 * z))
        .text_size(px(11.0 * z))
        .font_weight(FontWeight::BOLD)
        .text_color(c.ink)
        .whitespace_nowrap()
        .child(icon(Lucide::Check, 12.0 * z, c.ink))
        .child(ui::say(cx, Str::InboxChosen))
}

pub(crate) fn prompt_box(text: impl IntoElement, z: f32, c: &Colors) -> Div {
    div()
        .px(px(8.0 * z))
        .py(px(6.0 * z))
        .rounded(px(6.0 * z))
        .border_1()
        .border_color(c.line)
        .bg(c.bg)
        .text_size(px(SMALL * z))
        .text_color(c.ink)
        .font_family(fonts::TERMINAL_FAMILY)
        .child(text)
}

pub(crate) fn notice_badges(asks: usize, dones: usize, z: f32, c: &Colors) -> Vec<Div> {
    [(WaitTone::Ask, asks), (WaitTone::Done, dones)]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(tone, n)| {
            let glyph = match tone {
                WaitTone::Ask => Lucide::CircleQuestionMark,
                WaitTone::Done => Lucide::CircleCheck,
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(2.0 * z))
                .h(px(18.0 * z))
                .pl(px(4.0 * z))
                .pr(px(6.0 * z))
                .rounded(px(9.0 * z))
                .bg(tone_of(tone, c))
                .text_color(tone_ink(tone, c))
                .text_size(px(11.0 * z))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon(glyph, 12.0 * z, tone_ink(tone, c)))
                .child(n.to_string())
        })
        .collect()
}
