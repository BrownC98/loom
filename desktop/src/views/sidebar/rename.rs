use gpui::{Context, Entity, FontWeight, ParentElement, Styled, Subscription, Window, div, px};

use super::tree::{Body, Key};
use super::{Sidebar, acts, calls};
use crate::strings::Str;
use crate::theme::Colors;
use crate::views::cardlink;
use crate::views::field::{self, Field, FieldEvent};

pub(super) struct Renaming {
    pub key: Key,
    ticket: u64,
    pub field: Entity<Field>,
    name: String,
    pub error: Option<String>,
    ended: bool,
    _events: Subscription,
    _blur: Subscription,
}

impl Sidebar {
    pub(super) fn start_rename(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row_of(&key, cx).filter(acts::can_rename) else {
            return;
        };
        let name = match &row.body {
            Body::Node { name, .. } => name.clone(),
            Body::Group {
                branch: Some(branch),
                ..
            } => branch.clone(),
            _ => return,
        };
        let field = field::new(Str::RenameLabel, cx);
        field.update(cx, |field, cx| field.set_text(&name, cx));
        let handle = field.read(cx).handle();
        window.focus(&handle, cx);
        let events =
            cx.subscribe_in(
                &field,
                window,
                |this, _, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Enter => this.end_rename(true, true, window, cx),
                    FieldEvent::Escape => this.end_rename(false, true, window, cx),
                    FieldEvent::Changed => {
                        if let Some(r) = &mut this.renaming {
                            r.error = None;
                        }
                        cx.notify();
                    }
                    FieldEvent::Step(_) => {}
                },
            );
        let blur = cx.on_blur(&handle, window, |this, window, cx| {
            this.end_rename(true, false, window, cx)
        });
        self.renames += 1;
        self.renaming = Some(Renaming {
            key,
            ticket: self.renames,
            field,
            name,
            error: None,
            ended: false,
            _events: events,
            _blur: blur,
        });
        cx.notify();
    }

    fn end_rename(&mut self, save: bool, refocus: bool, _: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &mut self.renaming else { return };
        if r.ended {
            return;
        }
        r.ended = true;
        let value = r.field.read(cx).text().to_owned();
        if !save || value.trim() == r.name {
            self.renaming = None;
            self.refocus = refocus;
            cx.notify();
            return;
        }
        let key = r.key.clone();
        let ticket = r.ticket;
        let store = self.store.clone();
        let on = key.clone();
        let work = move |url: &str| match &on {
            Key::Node(id) => cardlink::rename(url, *id, &value),
            Key::Group(repo, path) => calls::rename_worktree(url, *repo, path, &value),
            _ => Ok(()),
        };
        calls::run(&store, cx, work, move |this, done, cx| {
            match done {
                Ok(()) => {
                    this.store.update(cx, |store, cx| store.reload(cx));
                    if this.renaming.as_ref().is_some_and(|r| r.ticket == ticket) {
                        this.renaming = None;
                        this.refocus = refocus;
                    }
                }
                Err(text) => {
                    if let Some(r) = this.renaming.as_mut().filter(|r| r.ticket == ticket) {
                        r.ended = false;
                        r.error = Some(text);
                    } else {
                        this.refusal = Some((key, text));
                    }
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn editor(&self, r: &Renaming, c: &Colors) -> gpui::Div {
        div()
            .flex_none()
            .flex_basis(px(160.0))
            .min_w(px(96.0))
            .h(px(22.0))
            .px(px(6.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(if r.error.is_some() { c.err } else { c.accent })
            .bg(c.bg)
            .font_weight(FontWeight::SEMIBOLD)
            .child(r.field.clone())
    }
}
