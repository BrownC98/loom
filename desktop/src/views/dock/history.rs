use api::graph::events::ClosedBy;
use api::graph::waits::{Closed, ClosedWait, WaitKind};
use gpui::{Context, Div, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};

use super::Dock;
use super::body;
use super::questions::kind_label;
use super::ui::{self, GAP, PAD, SMALL};
use crate::strings::Str;

const TIME: &str = "%Y/%m/%d %H:%M:%S";

fn closed_label(how: &Closed) -> Str {
    match how {
        Closed::Answered { .. } => Str::InboxClosedAnswered,
        Closed::Instructed { .. } => Str::InboxClosedInstructed,
        Closed::Confirmed => Str::InboxClosedConfirmed,
        Closed::NodeRemoved => Str::InboxClosedNodeRemoved,
        Closed::Unanswered { by } => match by {
            ClosedBy::Human => Str::InboxClosedByHuman,
            ClosedBy::Agent => Str::InboxClosedByAgent,
            ClosedBy::Restart => Str::InboxClosedByRestart,
            ClosedBy::Exit => Str::InboxClosedByExit,
        },
    }
}

impl Dock {
    pub(super) fn load_history(&mut self, cx: &mut Context<Self>) {
        let task = self
            .store
            .update(cx, |store, cx| store.closed_waits(None, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                match result {
                    Ok(page) => {
                        let newest = dock.history.rows.first().map_or(0, |row| row.seq);
                        let mut fresh: Vec<_> =
                            page.waits.into_iter().filter(|w| w.seq > newest).collect();
                        fresh.append(&mut dock.history.rows);
                        dock.history.rows = fresh;
                        if !dock.history.loaded {
                            dock.history.more = page.more;
                        }
                        dock.history.loaded = true;
                        dock.history.error = None;
                    }
                    Err(error) => dock.history.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn more_history(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.history.rows.last().map(|row| row.seq) else {
            return;
        };
        let task = self
            .store
            .update(cx, |store, cx| store.closed_waits(Some(last), cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                match result {
                    Ok(page) => {
                        dock.history
                            .rows
                            .extend(page.waits.into_iter().filter(|w| w.seq < last));
                        dock.history.more = page.more;
                    }
                    Err(error) => dock.history.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn history_list(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        if let (false, Some(error)) = (self.history.loaded, &self.history.error) {
            return ui::note(error.clone(), true, cx);
        }
        if self.history.loaded && self.history.rows.is_empty() {
            return ui::note(ui::say(cx, Str::InboxHistoryEmpty).to_owned(), false, cx);
        }
        let mut list = div()
            .flex()
            .flex_col()
            .children(self.history.rows.iter().map(|row| self.closed_row(row, cx)));
        if self.history.more {
            list = list.child(
                div().p(px(PAD)).child(
                    ui::button("history-more", cx)
                        .child(ui::say(cx, Str::InboxHistoryMore))
                        .on_click(cx.listener(|dock, _, _, cx| dock.more_history(cx))),
                ),
            );
        }
        if let Some(error) = &self.history.error {
            list = list.child(ui::note(error.clone(), true, cx));
        }
        list.text_color(c.ink)
    }

    fn closed_row(&self, closed: &ClosedWait, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let wait = &closed.wait.wait;
        let seq = closed.seq as usize;
        let name = match closed.node.id {
            Some(node) => ui::link(("history-node", seq), closed.node.name.clone(), cx)
                .on_click(cx.listener(move |dock, _, _, cx| dock.jump(node, cx)))
                .into_any_element(),
            None => div()
                .text_color(c.ink_3)
                .child(closed.node.name.clone())
                .into_any_element(),
        };
        let published = ui::say_with(
            cx,
            Str::InboxPublished,
            &[("time", &ui::clock(wait.since, TIME))],
        );
        let top = div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .text_size(px(SMALL))
            .text_color(c.ink_3)
            .child(ui::say(cx, kind_label(wait.kind)))
            .children((wait.kind == WaitKind::Question).then(|| wait.id.to_string()))
            .child(name)
            .child(div().flex_1().child(published));
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .mx(px(PAD))
            .mb(px(10.0))
            .p(px(PAD))
            .rounded(px(8.0))
            .border_1()
            .border_color(c.line)
            .bg(c.surface)
            .child(top);
        if let Some(text) = &wait.instruction {
            row = row.child(
                div()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(ui::say_with(cx, Str::InboxInstruction, &[("text", text)])),
            );
        }
        if !wait.text.is_empty() || wait.doc.is_some() {
            row = row.child(body::text(
                ("history-text", seq),
                &wait.parts,
                &wait.text,
                wait.doc.as_ref(),
                cx,
            ));
        }
        if !wait.choices.is_empty() {
            row = row.child(div().flex().flex_wrap().gap(px(GAP)).children(
                wait.choices.iter().map(|choice| {
                    let chosen = closed.chosen == Some(choice.key);
                    let mut chip = div()
                        .flex()
                        .gap(px(GAP))
                        .px(px(10.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(if chosen { c.accent } else { c.line_strong })
                        .text_size(px(SMALL))
                        .text_color(if closed.chosen.is_some() && !chosen {
                            c.ink_3
                        } else {
                            c.ink
                        })
                        .child(div().text_color(c.ink_3).child(choice.key.to_string()))
                        .child(choice.label.clone());
                    if choice.recommended {
                        chip = chip.child(
                            div()
                                .text_color(c.warn)
                                .child(ui::say(cx, Str::InboxRecommended)),
                        );
                    }
                    if chosen {
                        chip = chip.child(
                            div()
                                .text_color(c.accent)
                                .child(format!("\u{2713} {}", ui::say(cx, Str::InboxChosen))),
                        );
                    }
                    chip
                }),
            ));
        }
        let said = match &closed.how {
            Closed::Answered { text } | Closed::Instructed { text }
                if closed.chosen.map(String::from).as_deref() != Some(text.as_str()) =>
            {
                Some(text.clone())
            }
            _ => None,
        };
        row.child(
            div()
                .flex()
                .flex_col()
                .text_size(px(SMALL))
                .text_color(c.ink_2)
                .child(format!(
                    "{} {}",
                    ui::say(cx, closed_label(&closed.how)),
                    ui::clock(closed.closed_at, TIME)
                ))
                .children(said),
        )
    }
}
