//! IME composition in the dock terminal survives a selection change.

use gpui::{AppContext, EntityInputHandler, TestAppContext};

use super::{Dock, Tab, dock};
use crate::fixture::{BASE, snapshot};
use crate::store::Store;
use crate::views::inputs::Inputs;
use crate::views::pictures::Shelf;

#[gpui::test]
fn dock_keeps_its_terminal_while_composing(cx: &mut TestAppContext) {
    let (dock, cx) = cx.add_window_view(|window, cx| {
        cx.set_global(crate::theme::Theme::resolve(&Default::default(), true));
        let store = cx.new(|cx| Store::new(Err("test".to_owned()), window, cx));
        store.update(cx, |store, cx| {
            store.fixture(BASE, snapshot(Some("simple")), cx);
            store.select(1, cx);
        });
        let shelf = cx.new(|_| Shelf::default());
        let inputs = cx.new(|_| Inputs::default());
        let dock = dock(store.clone(), shelf, inputs, window, cx);
        dock.update(cx, |dock, _| dock.tab = Tab::Terminal);
        Holder { store, dock }
    });
    let held = |cx: &mut gpui::VisualTestContext| {
        dock.read_with(cx, |holder, cx| {
            holder.dock.read(cx).terminal.as_ref().map(|(id, _)| *id)
        })
    };
    cx.run_until_parked();
    assert_eq!(held(cx), Some(1));
    cx.update(|window, cx| {
        let view = dock
            .read(cx)
            .dock
            .read(cx)
            .terminal
            .as_ref()
            .unwrap()
            .1
            .clone();
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "ㅂ", None, window, cx)
        });
    });
    let store = dock.read_with(cx, |holder, _| holder.store.clone());
    store.update(cx, |store, cx| store.select(2, cx));
    cx.run_until_parked();
    assert_eq!(held(cx), Some(1), "the dock keeps the composing terminal");
}

struct Holder {
    store: gpui::Entity<Store>,
    dock: gpui::Entity<Dock>,
}

impl gpui::Render for Holder {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled};
        gpui::div().size_full().child(self.dock.clone())
    }
}
