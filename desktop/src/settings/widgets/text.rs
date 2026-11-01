use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, div,
};

use crate::views::field::{self, Field, FieldEvent};

pub enum TextEvent {
    Commit(String),
}

pub struct Text {
    field: Entity<Field>,
    committed: String,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TextEvent> for Text {}

impl Text {
    pub fn new(hint: &str, secret: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = field::boxed(hint, secret, false, cx);
        let focus = field.read(cx).handle();
        let subs =
            vec![
                cx.subscribe_in(&field, window, |this, _, event: &FieldEvent, window, cx| {
                    match event {
                        FieldEvent::Enter => this.commit(cx),
                        FieldEvent::Escape => {
                            this.revert(cx);
                            window.blur(cx);
                        }
                        _ => {}
                    }
                }),
                cx.on_blur(&focus, window, |this, _, cx| this.commit(cx)),
            ];
        Self {
            field,
            committed: String::new(),
            _subs: subs,
        }
    }

    pub fn show(&mut self, value: &str, window: &Window, cx: &mut Context<Self>) {
        if self.committed == value {
            return;
        }
        self.committed = value.to_owned();
        if !self.field.read(cx).focused(window) {
            self.field.update(cx, |field, cx| field.set_text(value, cx));
        }
    }

    pub fn reset(&mut self, value: &str, cx: &mut Context<Self>) {
        self.committed = value.to_owned();
        self.field.update(cx, |field, cx| field.set_text(value, cx));
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let text = self.field.read(cx).text().to_owned();
        if text != self.committed {
            self.committed = text.clone();
            cx.emit(TextEvent::Commit(text));
        }
    }

    fn revert(&mut self, cx: &mut Context<Self>) {
        let committed = self.committed.clone();
        self.field
            .update(cx, |field, cx| field.set_text(&committed, cx));
    }
}

impl Render for Text {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w_full().child(self.field.clone())
    }
}
