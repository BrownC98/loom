use std::rc::Rc;

use api::graph::NodeView;
use gpui::{
    Context, Div, Entity, MouseDownEvent, ParentElement, Styled, Subscription, Window, div,
};

use super::Dock;
use super::ui;
use crate::problem::Failure;
use crate::strings::Lang;
use crate::strings::Str;
use crate::views::card::{self, Actions, Face, Press};
use crate::views::cardlink;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::terminal;

pub(super) struct Rename {
    node: u32,
    ticket: u64,
    field: Entity<Field>,
    busy: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl Dock {
    fn begin_rename(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
            .map(|n| n.name.clone())
        else {
            return;
        };
        let field = field::new(Str::RenameLabel, cx);
        field.update(cx, |field, cx| field.set_text(&name, cx));
        let handle = field.read(cx).handle();
        let subs = vec![
            cx.subscribe(&field, |this, _, event: &FieldEvent, cx| match event {
                FieldEvent::Enter => this.end_rename(true, cx),
                FieldEvent::Escape => this.end_rename(false, cx),
                FieldEvent::Changed => {
                    if let Some(r) = &mut this.renaming {
                        r.error = None;
                    }
                    cx.notify();
                }
                FieldEvent::Step(_) => {}
            }),
            cx.on_blur(&handle, window, |this, _, cx| this.end_rename(true, cx)),
        ];
        window.focus(&handle, cx);
        self.edits += 1;
        self.renaming = Some(Rename {
            node: id,
            ticket: self.edits,
            field,
            busy: false,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn end_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(r) = &mut self.renaming else {
            return;
        };
        if r.busy {
            return;
        }
        let id = r.node;
        let value = r.field.read(cx).text().to_owned();
        let unchanged = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
            .is_none_or(|n| n.name == value.trim());
        if !commit || unchanged {
            self.renaming = None;
            cx.notify();
            return;
        }
        r.busy = true;
        let ticket = r.ticket;
        cardlink::run(
            &self.store,
            cx,
            move |base| cardlink::rename(base, id, &value),
            move |this, result, cx| {
                if this.renaming.as_ref().is_none_or(|r| r.ticket != ticket) {
                    return;
                }
                match result {
                    Ok(()) => this.renaming = None,
                    Err(text) => {
                        if let Some(r) = &mut this.renaming {
                            r.busy = false;
                            r.error = Some(text);
                        }
                    }
                }
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    fn card_call(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&str) -> Result<(), Failure> + Send + 'static,
    ) {
        cardlink::run(&self.store, cx, work, |dock, result, cx| {
            if let Err(text) = result {
                dock.store.update(cx, |store, cx| store.tell(text, cx));
            }
        })
        .detach();
    }

    pub(super) fn terminal_panel(
        &mut self,
        node: &NodeView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = ui::colors(cx);
        let id = node.id.0;
        if self.terminal.as_ref().is_none_or(|(held, _)| *held != id) {
            self.terminal = self.store.read(cx).base().map(str::to_owned).map(|base| {
                let view = terminal::open(&self.inputs, &base, id, window, cx);
                view.update(cx, |view, cx| view.focus(window, cx));
                (id, view)
            });
        }
        let lang = *cx.global::<Lang>();
        let parent = node.parent.and_then(|parent| {
            self.store
                .read(cx)
                .snapshot()?
                .nodes
                .iter()
                .find(|n| n.id == parent)
                .map(|n| n.name.clone())
        });
        let press =
            |run: fn(&mut Self, u32, &MouseDownEvent, &mut Window, &mut Context<Self>)| -> Press {
                Rc::new(cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    run(this, id, e, window, cx)
                }))
            };
        let actions = Actions {
            anchors: self.anchors.clone(),
            steer: press(|this, id, _, _, cx| {
                this.card_call(cx, move |base| cardlink::steer_queued(base, id))
            }),
            bookmark: press(|this, id, _, _, cx| {
                let on = this
                    .store
                    .read(cx)
                    .snapshot()
                    .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
                    .is_some_and(|n| n.bookmark.is_none());
                this.card_call(cx, move |base| crate::link::set_bookmark(base, id, on))
            }),
            expand: press(|this, id, _, _, cx| this.store.update(cx, |store, cx| store.go(id, cx))),
            menu: press(|this, id, e, _, cx| {
                let (at, button) = (e.position, this.anchors.get(&format!("menu:{id}")));
                this.store
                    .update(cx, |store, cx| store.open_node_menu(id, at, button, cx))
            }),
            rename: press(|this, id, _, window, cx| this.begin_rename(id, window, cx)),
        };
        let face = Face {
            lang,
            node,
            parent: parent.as_deref(),
            rename: self
                .renaming
                .as_ref()
                .filter(|r| r.node == id)
                .map(|r| (&r.field, r.error.as_deref())),
            actions: &actions,
            collapse: false,
            width: 300.0,
        };
        let head = card::dock_head(&face, &c);
        div().size_full().flex().flex_col().child(head).children(
            self.terminal
                .as_ref()
                .map(|(_, view)| div().flex_1().min_h_0().child(view.clone())),
        )
    }
}
