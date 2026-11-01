use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, div,
};

use crate::views::field::{self, Field, FieldEvent, Form};

pub enum TextEvent {
    Commit(String),
}

pub struct Text {
    field: Entity<Field>,
    committed: String,
    shown: String,
    keep: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TextEvent> for Text {}

impl Text {
    pub fn new(hint: &str, secret: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(hint, secret, Form::Plain, window, cx)
    }

    pub fn mono(hint: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(hint, false, Form::Mono, window, cx)
    }

    fn build(
        hint: &str,
        secret: bool,
        form: Form,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = field::form(hint, secret, false, form, cx);
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
            shown: String::new(),
            keep: false,
            _subs: subs,
        }
    }

    pub fn keep_draft(mut self) -> Self {
        self.keep = true;
        self
    }

    pub fn show(&mut self, value: &str, window: &Window, cx: &mut Context<Self>) {
        if self.keep {
            if self.shown == value {
                return;
            }
            self.shown = value.to_owned();
        } else if self.committed == value {
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
        if self.keep {
            self.committed = self.shown.clone();
        }
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
