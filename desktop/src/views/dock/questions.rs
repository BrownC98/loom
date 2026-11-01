use std::collections::HashSet;

use api::graph::waits::{InputField, Prompt, PromptKey, ScreenWait, WaitKind, WaitTone};
use api::ids::{NodeId, WaitId};
use gpui::{
    AnyElement, Context, Div, Entity, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Task, div, px,
};

use super::body::{self, Target};
use super::field::{self, Field, FieldEvent};
use super::ui::{self, GAP, PAD, SMALL};
use super::{Dock, Inbox, Slot};
use crate::strings::Str;

const TIME: &str = "%Y/%m/%d %H:%M:%S";

pub fn kind_label(kind: WaitKind) -> Str {
    match kind {
        WaitKind::Question => Str::WaitQuestion,
        WaitKind::Approval => Str::WaitApproval,
        WaitKind::Trust => Str::WaitTrust,
        WaitKind::Auth => Str::WaitAuth,
        WaitKind::Input => Str::WaitInput,
        WaitKind::Done => Str::WaitDone,
    }
}

pub fn key_label(key: &PromptKey, index: usize) -> String {
    match key {
        PromptKey::Char(c) => c.to_string(),
        PromptKey::Line(text) => text.chars().next().map(String::from).unwrap_or_default(),
        PromptKey::Tab => "tab".to_owned(),
        PromptKey::BackTab => "\u{21e7}tab".to_owned(),
        PromptKey::Move(_) | PromptKey::Tick(_) | PromptKey::Reply(_) => (index + 1).to_string(),
        PromptKey::Enter => "\u{23ce}".to_owned(),
    }
}

struct Group {
    node: NodeId,
    name: String,
    waits: Vec<ScreenWait>,
}

impl Dock {
    fn groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let Some(snapshot) = self.store.read(cx).snapshot() else {
            return Vec::new();
        };
        let mut groups: Vec<Group> = snapshot
            .nodes
            .iter()
            .filter(|n| !n.waits.is_empty())
            .map(|n| {
                let mut waits = n.waits.clone();
                waits.sort_by_key(|w| w.wait.since);
                Group {
                    node: n.id,
                    name: n.name.clone(),
                    waits,
                }
            })
            .collect();
        groups.sort_by_key(|g| g.waits[0].wait.since);
        groups
    }

    pub(super) fn find_wait(&self, id: u32, cx: &Context<Self>) -> Option<ScreenWait> {
        let snapshot = self.store.read(cx).snapshot()?;
        snapshot
            .nodes
            .iter()
            .flat_map(|n| n.waits.iter())
            .find(|w| w.wait.id.0 == id)
            .cloned()
    }

    pub(super) fn sync(&mut self, cx: &mut Context<Self>) {
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
        let mut ids: Vec<u32> = live.into_iter().collect();
        ids.sort_unstable();
        let key = format!("{ids:?}");
        if key != self.open_key {
            self.open_key = key;
            if self.history.loaded {
                self.load_history(cx);
            }
        }
        cx.notify();
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
        if let Some(prompt) = &wait.wait.prompt {
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
        let sub = cx.subscribe(&entity, move |dock, _, event: &FieldEvent, cx| {
            if matches!(event, FieldEvent::Enter) {
                dock.submit(id, slot, cx);
            }
        });
        self.fields.insert((id, slot), (entity, sub));
    }

    pub(super) fn follow(&mut self, target: Target, cx: &mut Context<Self>) {
        match target {
            Target::Doc(id) => self.open_doc(id, cx),
            Target::Url(url) => {
                self.fault = ui::open_url(&url).err();
                cx.notify();
            }
        }
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
            let _ = this.update(cx, |dock, cx| {
                dock.busy.remove(&wait);
                match result {
                    Ok(()) => {
                        if let Some(field) = clear {
                            field.update(cx, |field, cx| field.clear(cx));
                        }
                    }
                    Err(error) => {
                        dock.notes.insert(wait, error);
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

    fn prompt_of(&self, wait: u32, cx: &Context<Self>) -> Option<Prompt> {
        self.find_wait(wait, cx)?.wait.prompt
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

    pub(super) fn questions(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let groups = self.groups(cx);
        let count: usize = groups.iter().map(|g| g.waits.len()).sum();
        let segment = |id: &'static str, label: String, on: bool, cx: &mut Context<Self>| {
            let button = ui::button(id, cx);
            if on {
                button.bg(c.accent_wash).border_color(c.accent)
            } else {
                button
            }
            .child(label)
        };
        let new_label = if count > 0 {
            format!("{} {count}", ui::say(cx, Str::InboxNew))
        } else {
            ui::say(cx, Str::InboxNew).to_owned()
        };
        let switch = div()
            .flex()
            .gap(px(GAP))
            .p(px(PAD))
            .child(
                segment("inbox-new", new_label, self.inbox == Inbox::New, cx).on_click(
                    cx.listener(|dock, _, _, cx| {
                        dock.inbox = Inbox::New;
                        cx.notify();
                    }),
                ),
            )
            .child(
                segment(
                    "inbox-history",
                    ui::say(cx, Str::InboxHistory).to_owned(),
                    self.inbox == Inbox::History,
                    cx,
                )
                .on_click(cx.listener(|dock, _, _, cx| {
                    dock.inbox = Inbox::History;
                    dock.load_history(cx);
                    cx.notify();
                })),
            );
        let list = match self.inbox {
            Inbox::History => self.history_list(cx).into_any_element(),
            Inbox::New if groups.is_empty() => {
                ui::note(ui::say(cx, Str::InboxEmpty).to_owned(), false, cx).into_any_element()
            }
            Inbox::New => div()
                .flex()
                .flex_col()
                .children(groups.iter().map(|g| self.group(g, cx)))
                .into_any_element(),
        };
        div().flex().flex_col().child(switch).child(list)
    }

    fn group(&self, group: &Group, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let node = group.node;
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(4.0))
            .border_t_1()
            .border_color(c.line)
            .child(
                ui::link(("go", node.0 as usize), group.name.clone(), cx)
                    .px(px(PAD))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .on_click(cx.listener(move |dock, _, _, cx| dock.jump(node, cx))),
            )
            .children(group.waits.iter().map(|w| self.entry(node, w, cx)))
    }

    fn entry(&self, node: NodeId, wait: &ScreenWait, cx: &mut Context<Self>) -> Div {
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
            ui::button(("close", id as usize), cx)
                .child(ui::say(cx, Str::InboxConfirm))
                .on_click(cx.listener(move |dock, _, _, cx| dock.dismiss(id, cx)))
        } else {
            ui::button(("close", id as usize), cx)
                .px(px(8.0))
                .child("\u{00d7}")
                .on_click(cx.listener(move |dock, _, _, cx| dock.dismiss(id, cx)))
        };
        let top = div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .text_size(px(SMALL))
            .text_color(c.ink_3)
            .child(ui::say(cx, kind_label(wait.wait.kind)))
            .children((wait.wait.kind == WaitKind::Question).then(|| wait.wait.id.to_string()))
            .child(div().flex_1().child(published))
            .child(close);
        let mut entry = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .mx(px(PAD))
            .mb(px(10.0))
            .p(px(PAD))
            .rounded(px(8.0))
            .border_1()
            .border_color(c.line)
            .bg(c.surface)
            .opacity(if busy { 0.6 } else { 1.0 })
            .child(top);
        if let Some(text) = &wait.wait.instruction {
            entry = entry.child(
                div()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(ui::say_with(cx, Str::InboxInstruction, &[("text", text)])),
            );
        }
        entry = entry.child(self.wait_body(node, wait, cx));
        if let Some(note) = self.notes.get(&id) {
            entry = entry.child(
                div()
                    .text_size(px(SMALL))
                    .text_color(c.err)
                    .child(note.clone()),
            );
        }
        entry
    }

    fn wait_body(&self, node: NodeId, wait: &ScreenWait, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let id = wait.wait.id.0;
        let done = matches!(wait.tone, WaitTone::Done);
        let mut body = div().flex().flex_col().gap(px(GAP));
        if let Some(prompt) = &wait.wait.prompt {
            body = body.child(self.prompt_body(id, prompt, cx));
        } else if !wait.wait.text.is_empty() || wait.wait.doc.is_some() {
            body = body.child(body::text(
                ("text", id as usize),
                &wait.wait.parts,
                &wait.wait.text,
                wait.wait.doc.as_ref(),
                cx,
            ));
        }
        if wait.wait.answerable {
            if !done && !wait.wait.choices.is_empty() {
                body = body.child(div().flex().flex_wrap().gap(px(GAP)).children(
                    wait.wait.choices.iter().map(|choice| {
                        let key = choice.key;
                        let mut button =
                            ui::button(("choice", id as usize * 256 + key as usize % 256), cx)
                                .child(div().text_color(c.ink_3).child(key.to_string()))
                                .child(choice.label.clone());
                        if choice.recommended {
                            button = button.border_color(c.warn).child(
                                div()
                                    .text_color(c.warn)
                                    .child(ui::say(cx, Str::InboxRecommended)),
                            );
                        }
                        button.on_click(
                            cx.listener(move |dock, _, _, cx| dock.reply(id, key.to_string(), cx)),
                        )
                    }),
                ));
            }
            if let Some((field, _)) = self.fields.get(&(id, Slot::Answer)) {
                body = body.child(self.send_row(id, Slot::Answer, field, cx));
            }
        } else if wait.wait.prompt.is_none() {
            body = body.child(
                ui::button(("terminal", id as usize), cx)
                    .child(ui::say(cx, Str::InboxToTerminal))
                    .on_click(cx.listener(move |dock, _, _, cx| dock.jump(node, cx))),
            );
        }
        body
    }

    fn send_row(&self, id: u32, slot: Slot, field: &Entity<Field>, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let slot_id = match slot {
            Slot::Answer => 0,
            Slot::Option(index) => index + 1,
            Slot::Field => 255,
        };
        div()
            .flex()
            .items_end()
            .gap(px(GAP))
            .child(div().flex_1().min_w_0().child(field.clone()))
            .child(
                ui::button(("send", id as usize * 256 + slot_id), cx)
                    .w(px(28.0))
                    .h(px(28.0))
                    .justify_center()
                    .border_color(c.warn)
                    .bg(c.warn)
                    .text_color(c.warn_ink)
                    .child("\u{2191}")
                    .on_click(cx.listener(move |dock, _, _, cx| dock.submit(id, slot, cx))),
            )
    }

    fn prompt_body(&self, id: u32, prompt: &Prompt, cx: &mut Context<Self>) -> AnyElement {
        let c = ui::colors(cx);
        let busy = self.busy.contains(&id);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(div().text_size(px(SMALL)).child(prompt.body.join("\n")));
        let plain: Vec<_> = prompt
            .options
            .iter()
            .enumerate()
            .filter(|(_, o)| !o.writes)
            .collect();
        if !plain.is_empty() {
            body = body.child(div().flex().flex_wrap().gap(px(GAP)).children(
                plain.into_iter().map(|(index, option)| {
                    let mut button = ui::button(("option", id as usize * 256 + index), cx)
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
                    button.on_click(cx.listener(move |dock, _, _, cx| dock.pick(id, index, cx)))
                }),
            ));
        }
        for (index, option) in prompt.options.iter().enumerate() {
            if let Some((field, _)) = option
                .writes
                .then(|| self.fields.get(&(id, Slot::Option(index))))
                .flatten()
            {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(SMALL))
                                .text_color(c.ink_3)
                                .child(format!(
                                    "{} {}",
                                    key_label(&option.key, index),
                                    option.label
                                )),
                        )
                        .child(self.send_row(id, Slot::Option(index), field, cx)),
                );
            }
        }
        if let Some((field, _)) = self.fields.get(&(id, Slot::Field)) {
            body = body.child(self.send_row(id, Slot::Field, field, cx));
        }
        body.into_any_element()
    }
}
