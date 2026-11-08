use std::time::Duration;

use api::graph::events::ClosedBy;
use api::graph::search::WaitSearch;
use api::graph::waits::{Closed, ClosedWait, WaitKind};
use gpui::{
    Context, Div, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};

use super::body;
use super::entry::{self, Choice, Look};
use super::find::{self, Find};
use super::questions::kind_label;
use super::ui::{self, GAP, SMALL};
use super::{Dock, Inbox};
use crate::strings::Str;

const TIME: &str = "%Y/%m/%d %H:%M:%S";
const TYPE_PAUSE: Duration = Duration::from_millis(250);

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
    pub(super) fn search_text(&self, cx: &Context<Self>) -> String {
        self.search.read(cx).text().trim().to_owned()
    }

    pub(super) fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search_text(cx);
        self.find.follow(WaitSearch::new(&query).as_ref());
        cx.notify();
        if self.inbox != Inbox::History || (self.history.loaded && query == self.history.query) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPE_PAUSE).await;
            let _ = this.update(cx, |dock, cx| {
                if dock.search_text(cx) == query {
                    dock.load_history(cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn load_history(&mut self, cx: &mut Context<Self>) {
        let query = self.search_text(cx);
        let wanted = query.clone();
        let task = self
            .store
            .update(cx, |store, cx| store.closed_waits(wanted, None, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                if dock.search_text(cx) != query {
                    return;
                }
                let fresh = !dock.history.loaded || dock.history.query != query;
                match result {
                    Ok(page) if fresh => {
                        dock.history.rows = page.waits;
                        dock.history.more = page.more;
                        dock.history.query = query;
                        dock.history.loaded = true;
                        dock.history.error = None;
                        dock.find.restart();
                    }
                    Ok(page) => {
                        let newest = dock.history.rows.first().map_or(0, |row| row.seq);
                        let mut fresh: Vec<_> =
                            page.waits.into_iter().filter(|w| w.seq > newest).collect();
                        fresh.append(&mut dock.history.rows);
                        dock.history.rows = fresh;
                        dock.history.error = None;
                    }
                    Err(error) => {
                        if fresh {
                            dock.history.rows.clear();
                            dock.history.more = false;
                            dock.history.loaded = false;
                        }
                        dock.history.error = Some(error);
                    }
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
        let query = self.history.query.clone();
        let wanted = query.clone();
        let task = self
            .store
            .update(cx, |store, cx| store.closed_waits(wanted, Some(last), cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                if dock.history.query != query {
                    return;
                }
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

    pub(super) fn history_list(&self, find: Option<&Find>, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        if let (true, Some(error)) = (self.history.rows.is_empty(), &self.history.error) {
            return ui::empty(error.clone(), true, cx);
        }
        if self.history.loaded && self.history.rows.is_empty() {
            let empty = match self.history.query.is_empty() {
                true => Str::InboxHistoryEmpty,
                false => Str::InboxNoMatch,
            };
            return ui::empty(ui::say(cx, empty).to_owned(), false, cx);
        }
        let mut list =
            div()
                .flex()
                .flex_col()
                .p(px(6.0))
                .children(self.history.rows.iter().enumerate().map(|(at, row)| {
                    let mut item = self.closed_row(row, find, cx);
                    if at > 0 {
                        item = item.border_t_1().border_color(c.line);
                    }
                    item
                }));
        if self.history.more {
            let hover = c.surface_hover;
            let ink = c.ink;
            list = list.child(
                div().flex().justify_center().p(px(8.0)).child(
                    div()
                        .id("history-more")
                        .flex()
                        .items_center()
                        .h(px(26.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .text_size(px(SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.ink_2)
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover).text_color(ink))
                        .child(ui::say(cx, Str::InboxHistoryMore))
                        .on_click(cx.listener(|dock, _, _, cx| dock.more_history(cx))),
                ),
            );
        }
        if let Some(error) = &self.history.error {
            list = list.child(
                div()
                    .px(px(8.0))
                    .text_size(px(SMALL))
                    .text_color(c.err)
                    .child(error.clone()),
            );
        }
        list.text_color(c.ink)
    }

    fn closed_row(&self, closed: &ClosedWait, find: Option<&Find>, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let screen = &closed.wait;
        let wait = &screen.wait;
        let tone = screen.tone;
        let seq = closed.seq as usize;
        let chip = (wait.kind == WaitKind::Question).then(|| {
            find::chip(
                find,
                wait.id.0,
                entry::qid(wait.id.to_string(), tone, 20.0, 1.0, &c),
            )
        });
        let name = match closed.node.id {
            Some(node) => {
                let (line, ink) = (c.line_strong, c.ink);
                div()
                    .id(("history-node", seq))
                    .min_w_0()
                    .truncate()
                    .text_size(px(ui::TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.ink)
                    .underline()
                    .text_decoration_color(line)
                    .cursor_pointer()
                    .hover(move |style| style.text_decoration_color(ink))
                    .child(find::line(find, "", &closed.node.name, ""))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |dock, _, _, cx| dock.jump(node, cx)))
                    .into_any_element()
            }
            None => div()
                .min_w_0()
                .truncate()
                .text_size(px(ui::TEXT))
                .text_color(c.ink_3)
                .child(find::line(find, "", &closed.node.name, ""))
                .into_any_element(),
        };
        let top = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(GAP))
            .gap_y(px(2.0))
            .child(entry::mark(tone, 1.0, &c))
            .child(
                div()
                    .flex_none()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(ui::say(cx, kind_label(wait.kind))),
            )
            .children(chip)
            .child(name)
            .child(entry::published(wait.since, 1.0, cx));
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .pt(px(8.0))
            .px(px(8.0))
            .pb(px(10.0))
            .child(top);
        if let Some(text) = &wait.instruction {
            row = row.child(
                div()
                    .text_size(px(SMALL))
                    .text_color(c.ink)
                    .child(find::instruction(find, cx, text)),
            );
        }
        if !wait.text.is_empty() || wait.doc.is_some() {
            row = row.child(div().text_size(px(ui::TEXT)).child(body::text(
                ("history-text", seq),
                &wait.parts,
                &wait.text,
                wait.doc.as_ref(),
                entry::tone_of(tone, &c),
                find,
                cx,
            )));
        }
        if !wait.choices.is_empty() {
            row = row.child(div().flex().flex_wrap().gap(px(GAP)).children(
                wait.choices.iter().map(|choice| {
                    let chosen = closed.chosen == Some(choice.key);
                    let look = if chosen {
                        Look::Chosen
                    } else if closed.chosen.is_some() {
                        Look::Passed
                    } else if choice.recommended {
                        Look::Recommended
                    } else {
                        Look::Plain
                    };
                    let mut face = entry::choice_face(
                        div(),
                        Choice {
                            key: choice.key.to_string(),
                            label: &choice.label,
                            recommended: choice.recommended,
                            look,
                            tone,
                        },
                        1.0,
                        cx,
                    );
                    if look == Look::Passed {
                        face = face.opacity(0.55);
                    }
                    face.children(chosen.then(|| entry::chosen_mark(1.0, cx)))
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
                .text_size(px(SMALL))
                .text_color(c.ink_2)
                .flex()
                .flex_wrap()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.ink)
                        .child(ui::say(cx, closed_label(&closed.how)).to_owned()),
                )
                .child(format!(" {}", ui::clock(closed.closed_at, TIME)))
                .children(said.map(|text| find::line(find, " \u{00b7} ", &text, ""))),
        )
    }
}
