use std::rc::Rc;

use api::graph::NodeView;
use gpui::{Context, Div, MouseDownEvent, ParentElement, Styled, Window, div};

use super::Dock;
use super::ui;
use crate::problem::Failure;
use crate::strings::Lang;
use crate::views::anchor::Anchors;
use crate::views::card::{self, Actions, Face, Press};
use crate::views::cardlink;
use crate::views::terminal;

impl Dock {
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
        let press = |run: fn(&mut Self, u32, &MouseDownEvent, &mut Context<Self>)| -> Press {
            Rc::new(cx.listener(move |this, e: &MouseDownEvent, _, cx| run(this, id, e, cx)))
        };
        let actions = Actions {
            anchors: Anchors::default(),
            steer: press(|this, id, _, cx| {
                this.card_call(cx, move |base| cardlink::steer_queued(base, id))
            }),
            bookmark: press(|this, id, _, cx| {
                let on = this
                    .store
                    .read(cx)
                    .snapshot()
                    .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
                    .is_some_and(|n| n.bookmark.is_none());
                this.card_call(cx, move |base| crate::link::set_bookmark(base, id, on))
            }),
            expand: press(|this, id, _, cx| this.store.update(cx, |store, cx| store.go(id, cx))),
            menu: press(|this, id, e, cx| {
                let at = e.position;
                this.store
                    .update(cx, |store, cx| store.open_node_menu(id, at, cx))
            }),
            rename: press(|_, _, _, _| {}),
        };
        let face = Face {
            lang,
            node,
            parent: parent.as_deref(),
            rename: None,
            actions: &actions,
            collapse: false,
            width: 300.0,
        };
        let head = card::head(&face, 1.0, &c);
        div().size_full().flex().flex_col().child(head).children(
            self.terminal
                .as_ref()
                .map(|(_, view)| div().flex_1().min_h_0().child(view.clone())),
        )
    }
}
