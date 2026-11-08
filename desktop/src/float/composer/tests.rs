//! IME composition: a background update must not move focus or stop the
//! painting of the focused input while it holds marked text.

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div,
};

use super::{Composer, composer};
use crate::canvas::view::CanvasView;
use crate::fixture::{BASE, snapshot};
use crate::store::Store;
use crate::views::compose;
use crate::views::inputs::Inputs;
use crate::views::pictures::Shelf;
use crate::views::terminal::TerminalView;

struct Screen {
    store: Entity<Store>,
    inputs: Entity<Inputs>,
    composer: Entity<Composer>,
    canvas: Entity<CanvasView>,
}

impl Render for Screen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.canvas.clone())
            .child(self.composer.clone())
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Screen>, &mut VisualTestContext) {
    let (screen, cx) = cx.add_window_view(|window, cx| {
        cx.set_global(crate::theme::Theme::resolve(&Default::default(), true));
        let store = cx.new(|cx| Store::new(Err("test".to_owned()), window, cx));
        store.update(cx, |store, cx| store.fixture(BASE, snapshot(None), cx));
        let inputs = cx.new(|_| Inputs::default());
        let shelf = cx.new(|_| Shelf::default());
        let canvas = cx.new(|cx| CanvasView::new(store.clone(), inputs.clone(), window, cx));
        let composer = composer(store.clone(), inputs.clone(), shelf, window, cx);
        Screen {
            store,
            inputs,
            composer,
            canvas,
        }
    });
    (screen, cx)
}

fn compose_in_field(screen: &Entity<Screen>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let composer = screen.read(cx).composer.clone();
        let field = composer.read(cx).field.clone();
        composer.update(cx, |composer, cx| composer.focus_field(window, cx));
        field.update(cx, |field, cx| {
            use gpui::EntityInputHandler;
            field.replace_and_mark_text_in_range(None, "ㅂ", None, window, cx);
        });
    });
}

fn field_focused(screen: &Entity<Screen>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| {
        let field = screen.read(cx).composer.read(cx).field.clone();
        field.read(cx).focused(window)
    })
}

#[gpui::test]
fn selection_change_moves_focus_when_not_composing(cx: &mut TestAppContext) {
    let (screen, cx) = open(cx);
    cx.run_until_parked();
    cx.update(|window, cx| {
        let composer = screen.read(cx).composer.clone();
        composer.update(cx, |composer, cx| composer.focus_field(window, cx));
    });
    let store = screen.read_with(cx, |screen, _| screen.store.clone());
    store.update(cx, |store, cx| store.select(2, cx));
    cx.run_until_parked();
    assert!(
        !field_focused(&screen, cx),
        "baseline: the terminal takes focus"
    );
}

#[gpui::test]
fn selection_change_keeps_focus_while_composing(cx: &mut TestAppContext) {
    let (screen, cx) = open(cx);
    cx.run_until_parked();
    compose_in_field(&screen, cx);
    assert!(cx.update(|window, cx| compose::busy(window, cx)));
    let store = screen.read_with(cx, |screen, _| screen.store.clone());
    store.update(cx, |store, cx| store.select(2, cx));
    cx.run_until_parked();
    assert!(field_focused(&screen, cx));
}

#[gpui::test]
fn field_stays_painted_while_composing(cx: &mut TestAppContext) {
    let (screen, cx) = open(cx);
    let (store, inputs) = screen.read_with(cx, |s, _| (s.store.clone(), s.inputs.clone()));
    store.update(cx, |store, cx| store.select(1, cx));
    let view = cx.update(|window, cx| {
        let view = cx.new(|cx| TerminalView::new(Err("offline".to_owned()), window, cx));
        crate::views::inputs::offer(&inputs, 1, &view, cx);
        view
    });
    cx.run_until_parked();
    compose_in_field(&screen, cx);
    cx.run_until_parked();
    let field = screen.read_with(cx, |s, cx| s.composer.read(cx).field.clone());
    field.update(cx, |field, _| field.painted());
    // The card leaves reach: the registry drops the terminal of the aimed node.
    drop(view);
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(field.update(cx, |field, _| field.painted()));
}
