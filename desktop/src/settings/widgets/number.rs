use std::ops::RangeInclusive;

use gpui::{
    AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::text::{Text, TextEvent};
use crate::views::dock::ui::button;

const FIELD_W: f32 = 56.0;
const BOX_W: f32 = 120.0;

pub enum NumberEvent {
    Commit(Option<i64>),
}

pub struct Number {
    text: Entity<Text>,
    range: RangeInclusive<i64>,
    value: Option<i64>,
    nullable: bool,
    buttons: bool,
    _sub: Subscription,
}

impl EventEmitter<NumberEvent> for Number {}

impl Number {
    pub fn new(
        range: RangeInclusive<i64>,
        nullable: bool,
        buttons: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| Text::new("", false, window, cx));
        let sub = cx.subscribe(&text, |this, _, event: &TextEvent, cx| {
            let TextEvent::Commit(raw) = event;
            this.accept(raw, cx);
        });
        Self {
            text,
            range,
            value: None,
            nullable,
            buttons,
            _sub: sub,
        }
    }

    pub fn show(&mut self, value: Option<i64>, window: &Window, cx: &mut Context<Self>) {
        self.value = value;
        let shown = value.map(|n| n.to_string()).unwrap_or_default();
        self.text
            .update(cx, |text, cx| text.show(&shown, window, cx));
    }

    fn accept(&mut self, raw: &str, cx: &mut Context<Self>) {
        let raw = raw.trim();
        if raw.is_empty() && self.nullable {
            self.commit(None, cx);
            return;
        }
        match raw.parse::<i64>() {
            Ok(n) if self.range.contains(&n) => self.commit(Some(n), cx),
            _ => {
                let shown = self.value.map(|n| n.to_string()).unwrap_or_default();
                self.text.update(cx, |text, cx| text.reset(&shown, cx));
            }
        }
    }

    fn commit(&mut self, value: Option<i64>, cx: &mut Context<Self>) {
        if value != self.value {
            self.value = value;
            cx.emit(NumberEvent::Commit(value));
        }
    }

    fn step(&mut self, by: i64, cx: &mut Context<Self>) {
        let from = self.value.unwrap_or(*self.range.start());
        let next = (from + by).clamp(*self.range.start(), *self.range.end());
        self.commit(Some(next), cx);
    }
}

impl Render for Number {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div().flex().items_center().gap(px(6.0));
        if !self.buttons {
            return row.w(px(BOX_W)).child(self.text.clone());
        }
        row.child(
            button("number-minus", cx)
                .child("\u{2212}")
                .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
        )
        .child(div().w(px(FIELD_W)).child(self.text.clone()))
        .child(
            button("number-plus", cx)
                .child("+")
                .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
        )
    }
}
