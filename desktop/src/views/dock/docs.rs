use api::graph::docs::{DocGroup, DocRow};
use gpui::{
    Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};

use super::Dock;
use super::body::Target;
use super::ui::{self, SMALL, TEXT};
use crate::fonts;
use crate::strings::Lang;
use crate::strings::Str;
use crate::views::icons::{Lucide, icon};
use crate::views::tooltip::tip;

const GROUPS: [DocGroup; 2] = [DocGroup::Mockup, DocGroup::Report];

fn kind_label(kind: DocGroup) -> Str {
    match kind {
        DocGroup::Mockup => Str::DocsKindMockup,
        DocGroup::Report => Str::DocsKindReport,
    }
}

impl Dock {
    pub(super) fn load_docs(&mut self, cx: &mut Context<Self>) {
        self.docs.order += 1;
        let order = self.docs.order;
        let task = self.store.update(cx, |store, cx| store.list_docs(cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                if order <= dock.docs.loaded {
                    return;
                }
                dock.docs.loaded = order;
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
        let lang = *cx.global::<Lang>();
        let chips = GROUPS.iter().map(|&kind| {
            let on = !self.docs.hidden.contains(&kind);
            ui::chip(("doc-kind", kind as usize), on, cx)
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
        let mut panel = div()
            .size_full()
            .flex()
            .flex_col()
            .child(ui::hint(ui::say(cx, Str::DocsOrder), cx))
            .child(
                ui::filters(cx)
                    .children(chips)
                    .child(ui::search(&self.docs.query, cx)),
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
        panel.child(div().flex().flex_col().px(px(8.0)).py(px(4.0)).children(
            shown.into_iter().enumerate().map(|(index, doc)| {
                let openable = doc.source || doc.snapshot;
                let mut meta = vec![ui::say(cx, kind_label(doc.kind)).to_owned()];
                if openable {
                    meta.push(doc.folder.clone());
                    if !doc.source {
                        meta.push(ui::say(cx, Str::DocsLastCopy).to_owned());
                    }
                    if let Some(at) = doc.attached_at {
                        meta.push(ui::day_and_time(at, lang));
                    }
                } else {
                    meta.push(ui::say(cx, Str::DocsGone).to_owned());
                }
                let meta = meta.join(" \u{00b7} ");
                let id = doc.id.clone();
                let ink = if openable { c.ink } else { c.ink_3 };
                let title = if openable {
                    div()
                        .id(("doc", index))
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .min_w_0()
                        .cursor_pointer()
                        .text_color(c.ink)
                        .underline()
                        .child(div().min_w_0().truncate().child(doc.title.clone()))
                        .child(icon(Lucide::ExternalLink, 12.0, c.ink))
                        .on_click(cx.listener(move |dock, _, _, cx| {
                            dock.follow(Target::Doc(id.clone()), cx)
                        }))
                        .into_any_element()
                } else {
                    div()
                        .min_w_0()
                        .truncate()
                        .child(doc.title.clone())
                        .into_any_element()
                };
                let hover = c.surface_hover;
                div()
                    .id(("doc-row", index))
                    .flex()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(px(6.0))
                    .text_size(px(TEXT))
                    .hover(move |style| style.bg(hover))
                    .child(
                        div()
                            .w(px(44.0))
                            .flex_none()
                            .pt(px(1.0))
                            .text_size(px(SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_family(fonts::TERMINAL_FAMILY)
                            .text_color(if openable { c.ink_2 } else { c.ink_3 })
                            .child(doc.id.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .text_color(ink)
                            .child(title)
                            .child(
                                div()
                                    .id(("doc-meta", index))
                                    .truncate()
                                    .text_size(px(SMALL))
                                    .text_color(c.ink_3)
                                    .tooltip(tip(meta.clone()))
                                    .child(meta),
                            ),
                    )
            }),
        ))
    }
}
