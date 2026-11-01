use std::collections::HashMap;

use gpui::{App, Context, Entity, WeakEntity, Window};

use crate::views::terminal::{InputState, TerminalView};

#[derive(Default)]
pub struct Inputs {
    views: HashMap<u32, WeakEntity<TerminalView>>,
}

pub fn offer(registry: &Entity<Inputs>, node: u32, view: &Entity<TerminalView>, cx: &mut App) {
    let weak = view.downgrade();
    registry.update(cx, |inputs, cx| {
        inputs.views.insert(node, weak);
        cx.notify();
    });
    let id = view.entity_id();
    let registry = registry.downgrade();
    cx.observe_release(view, move |_, cx| {
        let _ = registry.update(cx, |inputs, cx| {
            if inputs
                .views
                .get(&node)
                .is_some_and(|held| held.entity_id() == id)
            {
                inputs.views.remove(&node);
                cx.notify();
            }
        });
    })
    .detach();
}

impl Inputs {
    fn view(&self, node: u32) -> Option<Entity<TerminalView>> {
        self.views.get(&node)?.upgrade()
    }

    pub fn state(&self, node: u32, cx: &App) -> Option<InputState> {
        Some(self.view(node)?.read(cx).input_state())
    }

    pub fn deliver(&self, node: u32, text: &str, cx: &mut Context<Self>) -> Option<InputState> {
        let view = self.view(node)?;
        let state = view.read(cx).input_state();
        if state == InputState::Open {
            view.update(cx, |view, cx| view.deliver(text, cx));
        }
        Some(state)
    }

    pub fn focus(&self, node: u32, window: &mut Window, cx: &mut App) {
        if let Some(view) = self.view(node) {
            view.update(cx, |view, cx| view.focus(window, cx));
        }
    }
}
