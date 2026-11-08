use api::graph::events::{Entry, EventRecord};
use gpui::{
    Context, Div, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{Tone, chip, line, pill, rework, since_short};
use crate::fonts;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::card::process_text;
use crate::views::cardlink;
use crate::views::ui;
use crate::views::worktree::{Closed, error, shell};

pub const WIDTH: f32 = 380.0;
pub const HEIGHT: f32 = 200.0;

pub struct StatusView {
    store: Entity<Store>,
    node: u32,
    history: Option<Result<Vec<EventRecord>, String>>,
    focus: FocusHandle,
}

impl EventEmitter<Closed> for StatusView {}

impl StatusView {
    pub fn new(
        store: Entity<Store>,
        node: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cardlink::run(
            &store,
            cx,
            move |base| cardlink::process_log(base, node),
            |this, result, cx| {
                this.history = Some(result);
                cx.notify();
            },
        )
        .detach();
        Self {
            store,
            node,
            history: None,
            focus,
        }
    }
}

fn heading(text: &str, c: &Colors) -> Div {
    div()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.ink_3)
        .mb(px(4.0))
        .child(text.to_owned())
}

fn changes(records: &[EventRecord], lang: Lang, c: &Colors) -> Vec<Div> {
    records
        .iter()
        .filter_map(|record| match &record.entry {
            Entry::ProcessChanged { from, to, .. } => Some((record.at, from, to)),
            _ => None,
        })
        .take(5)
        .map(|(at, from, to)| {
            div()
                .flex()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .font_family(fonts::TERMINAL_FAMILY)
                        .text_color(c.ink_3)
                        .child(ui::clock(at, "%H:%M")),
                )
                .child(format!(
                    "{} \u{2192} {}",
                    process_text(from, lang),
                    process_text(to, lang)
                ))
        })
        .collect()
}

impl Render for StatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let lang = *cx.global::<Lang>();
        let node = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.nodes.iter().find(|n| n.id.0 == self.node));
        let Some(node) = node else {
            return shell(&c, WIDTH).id("status-pop").track_focus(&self.focus);
        };
        let name = node.name.clone();
        let state = chip(&node.status, lang);
        let waiting = state
            .as_ref()
            .is_none_or(|chip| !matches!(chip.tone, Tone::Working | Tone::Result));
        let waits = waiting
            .then(|| line(node, None, lang))
            .flatten()
            .map(|text| {
                div()
                    .child(heading(ui::say(cx, Str::NodeStatusPopWaits), &c))
                    .child(text)
            });
        let since = (node.since > 0).then(|| {
            let time =
                Str::NodeStatusPopSince.with(lang, &[("time", &ui::clock(node.since, "%H:%M"))]);
            match since_short(node.since, lang) {
                Some(span) => format!("{time} \u{b7} {span}"),
                None => time,
            }
        });
        let history = match &self.history {
            Some(Ok(records)) => changes(records, lang, &c),
            _ => Vec::new(),
        };
        let failed = match &self.history {
            Some(Err(text)) => Some(error(text.clone(), &c)),
            _ => None,
        };
        let recent = (!history.is_empty()).then(|| {
            div()
                .child(heading(ui::say(cx, Str::NodeStatusPopHistory), &c))
                .child(div().flex().flex_col().gap(px(4.0)).children(history))
        });
        shell(&c, WIDTH)
            .id("status-pop")
            .track_focus(&self.focus)
            .gap(px(10.0))
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(Closed);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(node.name.clone()),
                    )
                    .children(state.map(|chip| pill(chip.text, chip.tone, 1.0, &c)))
                    .children(rework(node, lang).map(|chip| pill(chip.text, chip.tone, 1.0, &c))),
            )
            .children(since.map(|text| div().text_color(c.ink_2).child(text)))
            .children(waits)
            .children(recent)
            .children(failed)
            .child(
                div().flex().justify_end().child(
                    ui::button("status-events", cx)
                        .child(ui::say(cx, Str::NodeStatusPopEvents))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let name = name.clone();
                            this.store
                                .update(cx, |store, cx| store.show_events(name, cx));
                            cx.emit(Closed);
                        })),
                ),
            )
    }
}
