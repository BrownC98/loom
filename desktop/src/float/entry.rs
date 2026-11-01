use std::collections::HashSet;

use api::graph::text::TextPart;
use api::graph::waits::{InputField, Prompt, ScreenWait, WaitTone};
use api::ids::{NodeId, WaitId};
use gpui::{
    AnyElement, Context, Div, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Task, div, px,
};

use super::pill::Pill;
use crate::strings::Str;
use crate::views::dock::Slot;
use crate::views::dock::questions::{key_label, kind_label};
use crate::views::dock::ui::{self, GAP, PAD, SMALL};
use crate::views::field::{self, Field, FieldEvent};

const TIME: &str = "%Y/%m/%d %H:%M:%S";

fn plain(wait: &ScreenWait) -> String {
    if wait.wait.parts.is_empty() {
        return wait.wait.text.clone();
    }
    wait.wait
        .parts
        .iter()
        .map(|part| match part {
            TextPart::Text { text } => text.as_str(),
            TextPart::Doc { label } | TextPart::Link { label, .. } => label.as_str(),
        })
        .collect()
}

impl Pill {
    pub(super) fn sync_answers(&mut self, cx: &mut Context<Self>) {
        let waits: Vec<ScreenWait> = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| s.nodes.iter().flat_map(|n| n.waits.clone()).collect())
            .unwrap_or_default();
        let live: HashSet<u32> = waits.iter().map(|w| w.wait.id.0).collect();
        self.fields.retain(|(id, _), _| live.contains(id));
        self.notes.retain(|id, _| live.contains(id));
        self.busy.retain(|id| live.contains(id));
        for wait in &waits {
            self.ensure_fields(wait, cx);
        }
    }

    fn ensure_fields(&mut self, wait: &ScreenWait, cx: &mut Context<Self>) {
        let id = wait.wait.id.0;
        if wait.wait.answerable {
            let done = matches!(wait.tone, WaitTone::Done);
            let hint = ui::say(
                cx,
                if done {
                    Str::InboxFollowUpPlaceholder
                } else {
                    Str::InboxAnswerPlaceholder
                },
            );
            self.make_field(id, Slot::Answer, hint, false, true, cx);
        }
        let Some(prompt) = &wait.wait.prompt else {
            return;
        };
        for (index, option) in prompt.options.iter().enumerate() {
            if option.writes {
                self.make_field(id, Slot::Option(index), &option.label, false, false, cx);
            }
        }
        if let Some(kind) = prompt.field {
            let secret = kind == InputField::Secret;
            let hint = ui::say(
                cx,
                if secret {
                    Str::InboxSecretLabel
                } else {
                    Str::InboxTextLabel
                },
            );
            self.make_field(id, Slot::Field, hint, secret, false, cx);
        }
    }

    fn make_field(
        &mut self,
        id: u32,
        slot: Slot,
        hint: &str,
        secret: bool,
        multiline: bool,
        cx: &mut Context<Self>,
    ) {
        if self.fields.contains_key(&(id, slot)) {
            return;
        }
        let entity = field::boxed(hint, secret, multiline, cx);
        let sub = cx.subscribe(&entity, move |pill, _, event: &FieldEvent, cx| {
            if matches!(event, FieldEvent::Enter) {
                pill.submit(id, slot, cx);
            }
        });
        self.fields.insert((id, slot), (entity, sub));
    }

    fn find_wait(&self, id: u32, cx: &Context<Self>) -> Option<ScreenWait> {
        self.store
            .read(cx)
            .snapshot()?
            .nodes
            .iter()
            .flat_map(|n| n.waits.iter())
            .find(|w| w.wait.id.0 == id)
            .cloned()
    }

    fn prompt_of(&self, wait: u32, cx: &Context<Self>) -> Option<Prompt> {
        self.find_wait(wait, cx)?.wait.prompt
    }

    fn run(
        &mut self,
        wait: u32,
        clear: Option<Entity<Field>>,
        task: Task<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        self.busy.insert(wait);
        self.notes.remove(&wait);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |pill, cx| {
                pill.busy.remove(&wait);
                match result {
                    Ok(()) => {
                        if let Some(field) = clear {
                            field.update(cx, |field, cx| field.clear(cx));
                        }
                    }
                    Err(error) => {
                        pill.notes.insert(wait, error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit(&mut self, wait: u32, slot: Slot, cx: &mut Context<Self>) {
        if self.busy.contains(&wait) {
            return;
        }
        let Some(field) = self
            .fields
            .get(&(wait, slot))
            .map(|(field, _)| field.clone())
        else {
            return;
        };
        let text = field.read(cx).text().to_owned();
        let id = WaitId(wait);
        let store = self.store.clone();
        let task = match slot {
            Slot::Answer => {
                if field.read(cx).is_blank() {
                    return;
                }
                store.update(cx, |store, cx| store.answer_wait(id, text, cx))
            }
            Slot::Option(index) => {
                let Some(prompt) = self.prompt_of(wait, cx) else {
                    return;
                };
                store.update(cx, |store, cx| {
                    store.type_text(id, prompt, text, Some(index), cx)
                })
            }
            Slot::Field => {
                let Some(prompt) = self.prompt_of(wait, cx) else {
                    return;
                };
                store.update(cx, |store, cx| store.type_text(id, prompt, text, None, cx))
            }
        };
        self.run(wait, Some(field), task, cx);
    }

    fn pick(&mut self, wait: u32, option: usize, cx: &mut Context<Self>) {
        if self.busy.contains(&wait) {
            return;
        }
        let Some(prompt) = self.prompt_of(wait, cx) else {
            return;
        };
        let id = WaitId(wait);
        let task = self
            .store
            .update(cx, |store, cx| store.choose_option(id, prompt, option, cx));
        self.run(wait, None, task, cx);
    }

    fn reply(&mut self, wait: u32, text: String, cx: &mut Context<Self>) {
        if self.busy.contains(&wait) {
            return;
        }
        let id = WaitId(wait);
        let task = self
            .store
            .update(cx, |store, cx| store.answer_wait(id, text, cx));
        self.run(wait, None, task, cx);
    }

    fn dismiss(&mut self, wait: u32, cx: &mut Context<Self>) {
        let id = WaitId(wait);
        let task = self
            .store
            .update(cx, |store, cx| store.dismiss_wait(id, cx));
        self.run(wait, None, task, cx);
    }

    pub(super) fn jump(&mut self, node: NodeId, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.select(node.0, cx));
    }

    pub(super) fn entry(
        &self,
        node: NodeId,
        wait: &ScreenWait,
        z: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = ui::colors(cx);
        let id = wait.wait.id.0;
        let done = matches!(wait.tone, WaitTone::Done);
        let busy = self.busy.contains(&id);
        let published = ui::say_with(
            cx,
            Str::InboxPublished,
            &[("time", &ui::clock(wait.wait.since, TIME))],
        );
        let close = if done {
            ui::button_at(("pill-close-wait", id as usize), z, cx)
                .child(ui::say(cx, Str::InboxConfirm))
        } else {
            ui::button_at(("pill-close-wait", id as usize), z, cx)
                .px(px(8.0 * z))
                .child("\u{00d7}")
        }
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |pill, _, _, cx| pill.dismiss(id, cx)));
        let top = div()
            .flex()
            .items_center()
            .gap(px(GAP * z))
            .text_size(px(SMALL * z))
            .text_color(c.ink_3)
            .child(ui::say(cx, kind_label(wait.wait.kind)))
            .children(
                (wait.wait.kind == api::graph::waits::WaitKind::Question)
                    .then(|| wait.wait.id.to_string()),
            )
            .child(div().flex_1().child(published))
            .child(close);
        let mut entry = div()
            .flex()
            .flex_col()
            .gap(px(GAP * z))
            .mx(px(PAD * z))
            .mb(px(10.0 * z))
            .p(px(PAD * z))
            .rounded(px(8.0 * z))
            .border_1()
            .border_color(c.line)
            .bg(c.surface)
            .opacity(if busy { 0.6 } else { 1.0 })
            .child(top);
        if let Some(text) = &wait.wait.instruction {
            entry = entry.child(
                div()
                    .text_size(px(SMALL * z))
                    .text_color(c.ink_3)
                    .child(ui::say_with(cx, Str::InboxInstruction, &[("text", text)])),
            );
        }
        entry = entry.child(self.wait_body(node, wait, z, cx));
        if let Some(note) = self.notes.get(&id) {
            entry = entry.child(
                div()
                    .text_size(px(SMALL * z))
                    .text_color(c.err)
                    .child(note.clone()),
            );
        }
        entry
    }

    fn wait_body(&self, node: NodeId, wait: &ScreenWait, z: f32, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let id = wait.wait.id.0;
        let done = matches!(wait.tone, WaitTone::Done);
        let mut body = div().flex().flex_col().gap(px(GAP * z));
        if let Some(prompt) = &wait.wait.prompt {
            body = body.child(self.prompt_body(id, prompt, z, cx));
        } else if !wait.wait.text.is_empty() || !wait.wait.parts.is_empty() {
            body = body.child(div().text_size(px(SMALL * z)).child(plain(wait)));
        }
        if wait.wait.answerable {
            if !done && !wait.wait.choices.is_empty() {
                body = body.child(div().flex().flex_wrap().gap(px(GAP * z)).children(
                    wait.wait.choices.iter().map(|choice| {
                        let key = choice.key;
                        let mut button = ui::button_at(
                            ("pill-choice", id as usize * 256 + key as usize % 256),
                            z,
                            cx,
                        )
                        .child(div().text_color(c.ink_3).child(key.to_string()))
                        .child(choice.label.clone());
                        if choice.recommended {
                            button = button.border_color(c.warn).child(
                                div()
                                    .text_color(c.warn)
                                    .child(ui::say(cx, Str::InboxRecommended)),
                            );
                        }
                        button
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |pill, _, _, cx| {
                                pill.reply(id, key.to_string(), cx)
                            }))
                    }),
                ));
            }
            if let Some((field, _)) = self.fields.get(&(id, Slot::Answer)) {
                body = body.child(self.send_row(id, Slot::Answer, field, z, cx));
            }
        } else if wait.wait.prompt.is_none() {
            body = body.child(
                ui::button_at(("pill-terminal", id as usize), z, cx)
                    .child(ui::say(cx, Str::InboxToTerminal))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |pill, _, _, cx| pill.jump(node, cx))),
            );
        }
        body
    }

    fn send_row(
        &self,
        id: u32,
        slot: Slot,
        field: &Entity<Field>,
        z: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = ui::colors(cx);
        let slot_id = match slot {
            Slot::Answer => 0,
            Slot::Option(index) => index + 1,
            Slot::Field => 255,
        };
        div()
            .flex()
            .items_end()
            .gap(px(GAP * z))
            .child(div().flex_1().min_w_0().child(field.clone()))
            .child(
                ui::button_at(("pill-send", id as usize * 256 + slot_id), z, cx)
                    .w(px(28.0 * z))
                    .h(px(28.0 * z))
                    .justify_center()
                    .border_color(c.warn)
                    .bg(c.warn)
                    .text_color(c.warn_ink)
                    .child("\u{2191}")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |pill, _, _, cx| pill.submit(id, slot, cx))),
            )
    }

    fn prompt_body(&self, id: u32, prompt: &Prompt, z: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = ui::colors(cx);
        let busy = self.busy.contains(&id);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(GAP * z))
            .child(div().text_size(px(SMALL * z)).child(prompt.body.join("\n")));
        let options: Vec<_> = prompt
            .options
            .iter()
            .enumerate()
            .filter(|(_, o)| !o.writes)
            .collect();
        if !options.is_empty() {
            body = body.child(div().flex().flex_wrap().gap(px(GAP * z)).children(
                options.into_iter().map(|(index, option)| {
                    let mut button =
                        ui::button_at(("pill-option", id as usize * 256 + index), z, cx)
                            .child(
                                div()
                                    .text_color(c.ink_3)
                                    .child(key_label(&option.key, index)),
                            )
                            .child(option.label.clone());
                    if index == 0 {
                        button = button.border_color(c.warn);
                    }
                    if busy {
                        button = button.opacity(0.5);
                    }
                    button
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |pill, _, _, cx| pill.pick(id, index, cx)))
                }),
            ));
        }
        for (index, option) in prompt.options.iter().enumerate() {
            if let Some((field, _)) = option
                .writes
                .then(|| self.fields.get(&(id, Slot::Option(index))))
                .flatten()
            {
                body =
                    body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0 * z))
                            .child(div().text_size(px(SMALL * z)).text_color(c.ink_3).child(
                                format!("{} {}", key_label(&option.key, index), option.label),
                            ))
                            .child(self.send_row(id, Slot::Option(index), field, z, cx)),
                    );
            }
        }
        if let Some((field, _)) = self.fields.get(&(id, Slot::Field)) {
            body = body.child(self.send_row(id, Slot::Field, field, z, cx));
        }
        body.into_any_element()
    }
}
