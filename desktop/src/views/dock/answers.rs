use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use api::graph::waits::{InputField, Prompt, ScreenWait, WaitKind, WaitTone};
use api::ids::{NodeId, WaitId};
use gpui::{
    AnyElement, App, Context, Div, Entity, Image, IntoElement, ObjectFit, ParentElement,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, Task, div, img, px,
};

use super::body::{self, Target};
use super::questions::{key_label, kind_label};
use super::ui::{self, GAP, PAD, SMALL};
use crate::link::Snapshot;
use crate::store::{BusyChanged, DraftChanged, Store};
use crate::strings::Str;
use crate::views::field::{self, Field, FieldEvent};

const TIME: &str = "%Y/%m/%d %H:%M:%S";
const SHOT_HEIGHT: f32 = 96.0;
const SHOT_WIDTH: f32 = 128.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    Answer,
    Option(usize),
    Field,
}

struct Draft {
    field: Entity<Field>,
    secret: bool,
    _sub: Subscription,
}

pub(crate) struct Answers {
    store: Entity<Store>,
    fields: HashMap<(u32, Slot), Draft>,
    shown: HashMap<u32, Prompt>,
    notes: HashMap<u32, String>,
    images: HashMap<(u32, usize), Arc<Image>>,
    loading: HashSet<(u32, usize)>,
    _subs: Vec<Subscription>,
}

fn shown_prompts(snapshot: Option<&Snapshot>) -> HashMap<u32, Prompt> {
    snapshot
        .map(|s| {
            s.nodes
                .iter()
                .flat_map(|n| n.waits.iter())
                .filter_map(|w| Some((w.wait.id.0, w.wait.prompt.clone()?)))
                .collect()
        })
        .unwrap_or_default()
}

fn drop_absent_slots(fields: &mut HashMap<(u32, Slot), Draft>, id: u32, prompt: Option<&Prompt>) {
    fields.retain(|&(wait_id, slot), _| {
        wait_id != id
            || match slot {
                Slot::Answer => true,
                Slot::Option(index) => prompt
                    .and_then(|p| p.options.get(index))
                    .is_some_and(|o| o.writes),
                Slot::Field => prompt.is_some_and(|p| p.field.is_some()),
            }
    });
}

impl Answers {
    pub(crate) fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut answers = Self {
            store: store.clone(),
            fields: HashMap::new(),
            shown: HashMap::new(),
            notes: HashMap::new(),
            images: HashMap::new(),
            loading: HashSet::new(),
            _subs: Vec::new(),
        };
        answers
            ._subs
            .push(cx.observe(&store, |answers, _, cx| answers.sync(cx)));
        answers._subs.push(
            cx.subscribe(&store, |answers, store, event: &DraftChanged, cx| {
                let Some(draft) = answers.fields.get(&(event.0, event.1)) else {
                    return;
                };
                let text = store.read(cx).draft(event.0, event.1).to_owned();
                if draft.field.read(cx).text() != text {
                    draft
                        .field
                        .update(cx, |field, cx| field.set_text_at_end(&text, cx));
                }
            }),
        );
        answers
            ._subs
            .push(cx.subscribe(&store, |_, _, _: &BusyChanged, cx| cx.notify()));
        answers.sync(cx);
        answers
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let waits: Vec<ScreenWait> = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| s.nodes.iter().flat_map(|n| n.waits.clone()).collect())
            .unwrap_or_default();
        let live: HashSet<u32> = waits.iter().map(|w| w.wait.id.0).collect();
        self.shown = shown_prompts(self.store.read(cx).snapshot());
        self.fields.retain(|(id, _), _| live.contains(id));
        self.notes.retain(|id, _| live.contains(id));
        self.images.retain(|(id, _), _| live.contains(id));
        self.loading.retain(|(id, _)| live.contains(id));
        self.store
            .update(cx, |store, cx| store.keep_waits(&live, cx));
        for wait in &waits {
            self.ensure_fields(wait, cx);
            self.load_images(wait, cx);
        }
        cx.notify();
    }

    fn load_images(&mut self, wait: &ScreenWait, cx: &mut Context<Self>) {
        let id = wait.wait.id;
        for index in 0..wait.wait.images.len() {
            let key = (id.0, index);
            if self.images.contains_key(&key) || !self.loading.insert(key) {
                continue;
            }
            let task = self
                .store
                .update(cx, |store, cx| store.wait_image(id, index, cx));
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |answers, cx| {
                    if answers.loading.remove(&key) {
                        match result {
                            Ok(image) => {
                                answers.images.insert(key, image);
                            }
                            Err(error) => {
                                answers.notes.insert(id.0, error);
                            }
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
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
        let prompt = wait.wait.prompt.as_ref();
        drop_absent_slots(&mut self.fields, id, prompt);
        let Some(prompt) = prompt else {
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
        if self
            .fields
            .get(&(id, slot))
            .is_some_and(|draft| draft.secret == secret)
        {
            return;
        }
        let field = field::boxed(hint, secret, multiline, cx);
        if !secret {
            let kept = self.store.read(cx).draft(id, slot).to_owned();
            if !kept.is_empty() {
                field.update(cx, |field, cx| field.set_text_at_end(&kept, cx));
            }
        }
        let sub = cx.subscribe(
            &field,
            move |answers, field, event: &FieldEvent, cx| match event {
                FieldEvent::Enter => answers.submit(id, slot, cx),
                FieldEvent::Changed if !secret => {
                    let text = field.read(cx).text().to_owned();
                    answers
                        .store
                        .update(cx, |store, cx| store.set_draft(id, slot, &text, cx));
                }
                _ => {}
            },
        );
        self.fields.insert(
            (id, slot),
            Draft {
                field,
                secret,
                _sub: sub,
            },
        );
    }

    fn follow(&mut self, wait: u32, target: Target, cx: &mut Context<Self>) {
        let task = body::open(&self.store, target, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |answers, cx| {
                match result {
                    Ok(()) => answers.notes.remove(&wait),
                    Err(error) => answers.notes.insert(wait, error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn run(
        &mut self,
        wait: u32,
        clear: Option<Entity<Field>>,
        task: Task<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        self.store
            .update(cx, |store, cx| store.set_busy(wait, true, cx));
        self.notes.remove(&wait);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |answers, cx| {
                answers
                    .store
                    .update(cx, |store, cx| store.set_busy(wait, false, cx));
                match result {
                    Ok(()) => {
                        if let Some(field) = clear {
                            field.update(cx, |field, cx| field.clear(cx));
                        }
                    }
                    Err(error) => {
                        answers.notes.insert(wait, error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit(&mut self, wait: u32, slot: Slot, cx: &mut Context<Self>) {
        if self.store.read(cx).is_busy(wait) {
            return;
        }
        let Some(field) = self
            .fields
            .get(&(wait, slot))
            .map(|draft| draft.field.clone())
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
                let Some(prompt) = self.shown.get(&wait).cloned() else {
                    return;
                };
                store.update(cx, |store, cx| {
                    store.type_text(id, prompt, text, Some(index), cx)
                })
            }
            Slot::Field => {
                let Some(prompt) = self.shown.get(&wait).cloned() else {
                    return;
                };
                store.update(cx, |store, cx| store.type_text(id, prompt, text, None, cx))
            }
        };
        self.run(wait, Some(field), task, cx);
    }

    fn pick(&mut self, wait: u32, option: usize, cx: &mut Context<Self>) {
        if self.store.read(cx).is_busy(wait) {
            return;
        }
        let Some(prompt) = self.shown.get(&wait).cloned() else {
            return;
        };
        let id = WaitId(wait);
        let task = self
            .store
            .update(cx, |store, cx| store.choose_option(id, prompt, option, cx));
        self.run(wait, None, task, cx);
    }

    fn reply(&mut self, wait: u32, text: String, cx: &mut Context<Self>) {
        if self.store.read(cx).is_busy(wait) {
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
}

fn scaled(
    answers: &Entity<Answers>,
    id: u32,
    slot: Slot,
    z: f32,
    cx: &mut App,
) -> Option<Entity<Field>> {
    let field = answers.read(cx).fields.get(&(id, slot))?.field.clone();
    field.update(cx, |field, cx| field.set_scale(z, cx));
    Some(field)
}

pub(crate) fn entry(
    answers: &Entity<Answers>,
    node: NodeId,
    wait: &ScreenWait,
    z: f32,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let id = wait.wait.id.0;
    let done = matches!(wait.tone, WaitTone::Done);
    let busy = answers.read(cx).store.read(cx).is_busy(id);
    let published = ui::say_with(
        cx,
        Str::InboxPublished,
        &[("time", &ui::clock(wait.wait.since, TIME))],
    );
    let dismiss = {
        let answers = answers.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
            answers.update(cx, |answers, cx| answers.dismiss(id, cx));
        }
    };
    let close = if done {
        ui::button_at(("close", id as usize), z, cx).child(ui::say(cx, Str::InboxConfirm))
    } else {
        ui::button_at(("close", id as usize), z, cx)
            .px(px(8.0 * z))
            .child("\u{00d7}")
    }
    .on_click(dismiss);
    let top = div()
        .flex()
        .items_center()
        .gap(px(GAP * z))
        .text_size(px(SMALL * z))
        .text_color(c.ink_3)
        .child(ui::say(cx, kind_label(wait.wait.kind)))
        .children((wait.wait.kind == WaitKind::Question).then(|| wait.wait.id.to_string()))
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
    entry = entry.child(wait_body(answers, node, wait, z, cx));
    if let Some(note) = answers.read(cx).notes.get(&id) {
        entry = entry.child(
            div()
                .text_size(px(SMALL * z))
                .text_color(c.err)
                .child(note.clone()),
        );
    }
    entry
}

fn wait_body(
    answers: &Entity<Answers>,
    node: NodeId,
    wait: &ScreenWait,
    z: f32,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let id = wait.wait.id.0;
    let done = matches!(wait.tone, WaitTone::Done);
    let mut body = div().flex().flex_col().gap(px(GAP * z));
    let shots: Vec<Arc<Image>> = (0..wait.wait.images.len())
        .filter_map(|index| answers.read(cx).images.get(&(id, index)).cloned())
        .collect();
    if !shots.is_empty() {
        body = body.child(div().flex().flex_wrap().gap(px(GAP * z)).children(
            shots.into_iter().map(|image| {
                img(image)
                    .w(px(SHOT_WIDTH * z))
                    .h(px(SHOT_HEIGHT * z))
                    .object_fit(ObjectFit::Contain)
                    .rounded(px(4.0 * z))
            }),
        ));
    }
    if let Some(prompt) = &wait.wait.prompt {
        body = body.child(prompt_body(answers, id, prompt, z, cx));
    } else if !wait.wait.text.is_empty() || !wait.wait.parts.is_empty() || wait.wait.doc.is_some() {
        let owner = answers.clone();
        body = body.child(div().text_size(px(SMALL * z)).child(body::render(
            ("text", id as usize),
            &wait.wait.parts,
            &wait.wait.text,
            wait.wait.doc.as_ref(),
            cx,
            move |target, cx| owner.update(cx, |answers, cx| answers.follow(id, target, cx)),
        )));
    }
    if wait.wait.answerable {
        if !done && !wait.wait.choices.is_empty() {
            body = body.child(div().flex().flex_wrap().gap(px(GAP * z)).children(
                wait.wait.choices.iter().map(|choice| {
                    let key = choice.key;
                    let owner = answers.clone();
                    let mut button =
                        ui::button_at(("choice", id as usize * 256 + key as usize % 256), z, cx)
                            .child(div().text_color(c.ink_3).child(key.to_string()))
                            .child(choice.label.clone());
                    if choice.recommended {
                        button = button.border_color(c.warn).child(
                            div()
                                .text_color(c.warn)
                                .child(ui::say(cx, Str::InboxRecommended)),
                        );
                    }
                    button.on_click(move |_, _, cx| {
                        owner.update(cx, |answers, cx| answers.reply(id, key.to_string(), cx))
                    })
                }),
            ));
        }
        let field = scaled(answers, id, Slot::Answer, z, cx);
        if let Some(field) = field {
            body = body.child(send_row(answers, id, Slot::Answer, &field, z, cx));
        }
    } else if wait.wait.prompt.is_none() {
        let store = answers.read(cx).store.clone();
        body = body.child(
            ui::button_at(("terminal", id as usize), z, cx)
                .child(ui::say(cx, Str::InboxToTerminal))
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| store.select(node.0, cx));
                }),
        );
    }
    body
}

fn send_row(
    answers: &Entity<Answers>,
    id: u32,
    slot: Slot,
    field: &Entity<Field>,
    z: f32,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let owner = answers.clone();
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
            ui::button_at(("send", id as usize * 256 + slot_id), z, cx)
                .w(px(28.0 * z))
                .h(px(28.0 * z))
                .justify_center()
                .border_color(c.warn)
                .bg(c.warn)
                .text_color(c.warn_ink)
                .child("\u{2191}")
                .on_click(move |_, _, cx| {
                    owner.update(cx, |answers, cx| answers.submit(id, slot, cx))
                }),
        )
}

fn prompt_body(
    answers: &Entity<Answers>,
    id: u32,
    prompt: &Prompt,
    z: f32,
    cx: &mut App,
) -> AnyElement {
    let c = ui::colors(cx);
    let busy = answers.read(cx).store.read(cx).is_busy(id);
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(GAP * z))
        .child(div().text_size(px(SMALL * z)).child(prompt.body.join("\n")));
    let plain: Vec<_> = prompt
        .options
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.writes)
        .collect();
    if !plain.is_empty() {
        body = body.child(div().flex().flex_wrap().gap(px(GAP * z)).children(
            plain.into_iter().map(|(index, option)| {
                let owner = answers.clone();
                let mut button = ui::button_at(("option", id as usize * 256 + index), z, cx)
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
                button.on_click(move |_, _, cx| {
                    owner.update(cx, |answers, cx| answers.pick(id, index, cx))
                })
            }),
        ));
    }
    for (index, option) in prompt.options.iter().enumerate() {
        let field = option
            .writes
            .then(|| scaled(answers, id, Slot::Option(index), z, cx))
            .flatten();
        if let Some(field) = field {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0 * z))
                    .child(
                        div()
                            .text_size(px(SMALL * z))
                            .text_color(c.ink_3)
                            .child(format!(
                                "{} {}",
                                key_label(&option.key, index),
                                option.label
                            )),
                    )
                    .child(send_row(answers, id, Slot::Option(index), &field, z, cx)),
            );
        }
    }
    let field = scaled(answers, id, Slot::Field, z, cx);
    if let Some(field) = field {
        body = body.child(send_row(answers, id, Slot::Field, &field, z, cx));
    }
    body.into_any_element()
}
