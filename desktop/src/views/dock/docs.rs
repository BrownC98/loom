use api::graph::docs::{DocGroup, DocRow};
use gpui::{Context, Div, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};

use super::Dock;
use super::ui::{self, GAP, PAD, SMALL};
use crate::strings::Str;

const GROUPS: [DocGroup; 2] = [DocGroup::Mockup, DocGroup::Report];
const TIME: &str = "%m/%d %H:%M";

fn kind_label(kind: DocGroup) -> Str {
    match kind {
        DocGroup::Mockup => Str::DocsKindMockup,
        DocGroup::Report => Str::DocsKindReport,
    }
}

impl Dock {
    pub(super) fn load_docs(&mut self, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |store, cx| store.list_docs(cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                match result {
                    Ok(rows) => {
                        dock.docs.rows = Some(rows);
                        dock.docs.error = None;
                    }
                    Err(error) => dock.docs.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_doc(&mut self, id: String, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |store, cx| store.doc_url(&id, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await.and_then(|url| ui::open_url(&url));
            let _ = this.update(cx, |dock, cx| {
                dock.fault = result.err();
                cx.notify();
            });
        })
        .detach();
    }

    fn shown_docs<'a>(&'a self, rows: &'a [DocRow], cx: &Context<Self>) -> Vec<&'a DocRow> {
        let needle = self.docs.query.read(cx).text().trim().to_lowercase();
        rows.iter()
            .filter(|d| {
                !self.docs.hidden.contains(&d.kind)
                    && (needle.is_empty()
                        || d.id.to_lowercase().contains(&needle)
                        || d.title.to_lowercase().contains(&needle))
            })
            .collect()
    }

    pub(super) fn documents(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let chips = GROUPS.iter().map(|&kind| {
            let on = !self.docs.hidden.contains(&kind);
            let chip = ui::button(("doc-kind", kind as usize), cx);
            if on {
                chip.bg(c.accent_wash).border_color(c.accent)
            } else {
                chip
            }
            .child(ui::say(cx, kind_label(kind)))
            .on_click(cx.listener(move |dock, _, _, cx| {
                match dock.docs.hidden.iter().position(|&k| k == kind) {
                    Some(at) => {
                        dock.docs.hidden.remove(at);
                    }
                    None => dock.docs.hidden.push(kind),
                }
                cx.notify();
            }))
        });
        let mut panel = div().flex().flex_col().child(
            div()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .p(px(PAD))
                .child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(c.ink_3)
                        .child(ui::say(cx, Str::DocsOrder)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(GAP))
                        .children(chips)
                        .child(div().flex_1().min_w_0().child(self.docs.query.clone())),
                ),
        );
        if let Some(error) = &self.docs.error {
            panel = panel.child(ui::note(error.clone(), true, cx));
        }
        let Some(rows) = &self.docs.rows else {
            if self.docs.error.is_none() {
                panel = panel.child(ui::note(
                    ui::say(cx, Str::DocsReading).to_owned(),
                    false,
                    cx,
                ));
            }
            return panel;
        };
        let shown = self.shown_docs(rows, cx);
        if rows.is_empty() {
            return panel.child(ui::note(ui::say(cx, Str::DocsEmpty).to_owned(), false, cx));
        }
        if shown.is_empty() {
            return panel.child(ui::note(
                ui::say(cx, Str::DocsNoMatch).to_owned(),
                false,
                cx,
            ));
        }
        panel.children(shown.into_iter().enumerate().map(|(index, doc)| {
            let openable = doc.source || doc.snapshot;
            let mut meta = vec![ui::say(cx, kind_label(doc.kind)).to_owned()];
            if openable {
                meta.push(doc.folder.clone());
                if !doc.source {
                    meta.push(ui::say(cx, Str::DocsLastCopy).to_owned());
                }
                if let Some(at) = doc.attached_at {
                    meta.push(ui::clock(at, TIME));
                }
            } else {
                meta.push(ui::say(cx, Str::DocsGone).to_owned());
            }
            let id = doc.id.clone();
            let title = if openable {
                ui::link(("doc", index), doc.title.clone(), cx)
                    .on_click(cx.listener(move |dock, _, _, cx| dock.open_doc(id.clone(), cx)))
                    .into_any_element()
            } else {
                div()
                    .text_color(c.ink_3)
                    .child(doc.title.clone())
                    .into_any_element()
            };
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p(px(PAD))
                .border_b_1()
                .border_color(c.line)
                .child(
                    div()
                        .flex()
                        .gap(px(PAD))
                        .child(div().text_color(c.ink_3).child(doc.id.clone()))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(c.ink_3)
                        .child(meta.join(" \u{00b7} ")),
                )
        }))
    }
}
