use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use api::graph::waits::{InputField, Prompt, ScreenWait, WaitKind, WaitTone};
use api::ids::{NodeId, WaitId};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, ExternalPaths, FontWeight, Image,
    InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, Task, Window, div, img, px,
};

use super::blocks;
use super::body::{self, Target};
use super::entry::{self, Choice, Look};
use super::find::{self, Find};
use super::questions::{key_label, kind_label};
use super::ui::{self, GAP, SMALL};
use crate::store::{BusyChanged, DraftChanged, Slot, Store};
use crate::strings::Str;
use crate::theme::mix;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};
use crate::views::pictures::{self, Pictures, PicturesEvent, Shelf};
use crate::views::tooltip::tip;
use crate::views::viewer::Viewer;

const SHOT_HEIGHT: f32 = 44.0;
const SHOT_WIDTH: f32 = 64.0;

struct Draft {
    field: Entity<Field>,
    secret: bool,
    _sub: Subscription,
}

struct Attach {
    node: NodeId,
    pictures: Entity<Pictures>,
    _sub: Subscription,
}

type Done = Box<dyn FnOnce(&mut App)>;

enum Caption<'a> {
    Label(Str),
    Text(&'a str),
}

enum Text {
    Typed(String),
    Field(Entity<Field>),
}

impl Text {
    fn read(self, cx: &App) -> String {
        match self {
            Self::Typed(text) => text,
            Self::Field(field) => field.read(cx).text().to_owned(),
        }
    }
}

fn cleared(field: Entity<Field>) -> Done {
    Box::new(move |cx| field.update(cx, |field, cx| field.clear(cx)))
}

pub(crate) struct Answers {
    store: Entity<Store>,
    shelf: Entity<Shelf>,
    fields: HashMap<(u32, Slot), Draft>,
    shown: HashMap<u32, Prompt>,
    notes: HashMap<u32, String>,
    images: HashMap<(u32, usize), Arc<Image>>,
    loading: HashSet<(u32, usize)>,
    attached: HashMap<u32, Attach>,
    acted: HashSet<u32>,
    waiting: HashMap<u32, (Text, Option<Done>)>,
    viewer: Entity<Viewer>,
    zoomed: Option<u32>,
    shared: Option<Entity<Pictures>>,
    _subs: Vec<Subscription>,
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
    pub(crate) fn new(store: Entity<Store>, shelf: Entity<Shelf>, cx: &mut Context<Self>) -> Self {
        let mut answers = Self {
            store: store.clone(),
            shelf,
            fields: HashMap::new(),
            shown: HashMap::new(),
            notes: HashMap::new(),
            images: HashMap::new(),
            loading: HashSet::new(),
            attached: HashMap::new(),
            waiting: HashMap::new(),
            acted: HashSet::new(),
            viewer: cx.new(Viewer::new),
            zoomed: None,
            shared: None,
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

    pub(crate) fn share(&mut self, pictures: Entity<Pictures>) {
        self.shared = Some(pictures);
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let waits: Vec<(NodeId, ScreenWait)> = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| {
                s.nodes
                    .iter()
                    .flat_map(|n| n.waits.iter().map(|w| (n.id, w.clone())))
                    .collect()
            })
            .unwrap_or_default();
        let live: HashSet<u32> = waits.iter().map(|(_, w)| w.wait.id.0).collect();
        self.shown.retain(|id, _| live.contains(id));
        self.fields.retain(|(id, _), _| live.contains(id));
        self.notes.retain(|id, _| live.contains(id));
        self.images.retain(|(id, _), _| live.contains(id));
        self.loading.retain(|(id, _)| live.contains(id));
        self.attached.retain(|id, _| live.contains(id));
        self.waiting.retain(|id, _| live.contains(id));
        self.shelf.update(cx, |shelf, _| shelf.keep(&live));
        self.store
            .update(cx, |store, cx| store.keep_waits(&live, cx));
        for (node, wait) in &waits {
            self.ensure_pictures(*node, wait, cx);
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

    fn ensure_pictures(&mut self, node: NodeId, wait: &ScreenWait, cx: &mut Context<Self>) {
        let id = wait.wait.id.0;
        if !wait.wait.answerable {
            self.attached.remove(&id);
            if self.waiting.remove(&id).is_some() {
                self.store
                    .update(cx, |store, cx| store.set_busy(id, false, cx));
            }
            return;
        }
        if !self.attached.contains_key(&id) {
            let store = self.store.clone();
            let shared = self.shared.clone();
            let pictures = shared.clone().unwrap_or_else(|| {
                self.shelf
                    .update(cx, |shelf, cx| shelf.pictures(&store, node, id, cx))
            });
            let sub =
                cx.subscribe(
                    &pictures,
                    move |answers, _, event: &PicturesEvent, cx| match event {
                        PicturesEvent::Settled => answers.settled(id, cx),
                        _ if shared.is_some() => {}
                        PicturesEvent::Refused(text) => {
                            answers.notes.insert(id, text.clone());
                            cx.notify();
                        }
                        PicturesEvent::Other(name, _) => {
                            let text = ui::say_with(cx, Str::InboxNotImage, &[("name", name)]);
                            answers.notes.insert(id, text);
                            cx.notify();
                        }
                    },
                );
            self.attached.insert(
                id,
                Attach {
                    node,
                    pictures,
                    _sub: sub,
                },
            );
        }
    }

    fn settled(&mut self, wait: u32, cx: &mut Context<Self>) {
        if let Some((text, done)) = self.waiting.remove(&wait) {
            self.send(wait, text, done, cx);
        }
    }

    fn ensure_fields(&mut self, wait: &ScreenWait, cx: &mut Context<Self>) {
        let id = wait.wait.id.0;
        if wait.wait.answerable {
            let done = matches!(wait.tone, WaitTone::Done);
            let hint = if done {
                Str::InboxFollowUpPlaceholder
            } else {
                Str::InboxAnswerPlaceholder
            };
            self.make_field(id, Slot::Answer, Caption::Label(hint), false, true, cx);
            self.hook_files(id, cx);
        }
        let prompt = wait.wait.prompt.as_ref();
        drop_absent_slots(&mut self.fields, id, prompt);
        let Some(prompt) = prompt else {
            return;
        };
        for (index, option) in prompt.options.iter().enumerate() {
            if option.writes {
                self.make_field(
                    id,
                    Slot::Option(index),
                    Caption::Text(&option.label),
                    false,
                    false,
                    cx,
                );
            }
        }
        if let Some(kind) = prompt.field {
            let secret = kind == InputField::Secret;
            let hint = if secret {
                Str::InboxSecretLabel
            } else {
                Str::InboxTextLabel
            };
            self.make_field(id, Slot::Field, Caption::Label(hint), secret, false, cx);
        }
    }

    fn hook_files(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(attach) = self.attached.get(&id) else {
            return;
        };
        let pictures = attach.pictures.clone();
        if let Some(draft) = self.fields.get(&(id, Slot::Answer)) {
            draft.field.update(cx, |field, _| {
                field.on_files(move |entries, _, cx| {
                    pictures.update(cx, |pictures, cx| pictures.take_clipboard(entries, cx));
                });
            });
        }
    }

    fn make_field(
        &mut self,
        id: u32,
        slot: Slot,
        hint: Caption,
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
        let field = match hint {
            Caption::Label(label) => field::boxed_label(label, secret, multiline, cx),
            Caption::Text(text) => field::boxed(text, secret, multiline, cx),
        };
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

    fn zoom(&mut self, wait: u32, image: Arc<Image>, window: &mut Window, cx: &mut Context<Self>) {
        self.zoomed = Some(wait);
        self.viewer
            .update(cx, |viewer, cx| viewer.open(image, window, cx));
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

    pub(crate) fn mark_acted(&mut self, wait: u32) {
        self.acted.insert(wait);
    }

    pub(crate) fn unmark_acted(&mut self, wait: u32) {
        self.acted.remove(&wait);
    }

    pub(crate) fn take_acted(&mut self, wait: u32) -> bool {
        self.acted.remove(&wait)
    }

    fn run(
        &mut self,
        wait: u32,
        done: Option<Done>,
        task: Task<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        self.store
            .update(cx, |store, cx| store.set_busy(wait, true, cx));
        self.notes.remove(&wait);
        self.acted.insert(wait);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |answers, cx| {
                answers
                    .store
                    .update(cx, |store, cx| store.set_busy(wait, false, cx));
                match result {
                    Ok(()) => {
                        if let Some(done) = done {
                            done(cx);
                        }
                    }
                    Err(error) => {
                        answers.acted.remove(&wait);
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
                let bare = self
                    .attached
                    .get(&wait)
                    .is_none_or(|a| a.pictures.read(cx).is_empty());
                if field.read(cx).is_blank() && bare {
                    return;
                }
                self.send(wait, Text::Field(field.clone()), Some(cleared(field)), cx);
                return;
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
        self.run(wait, Some(cleared(field)), task, cx);
    }

    fn send(&mut self, wait: u32, text: Text, done: Option<Done>, cx: &mut Context<Self>) {
        let pictures = self.attached.get(&wait).map(|a| {
            a.pictures
                .update(cx, |pictures, cx| pictures.retarget(a.node, cx));
            a.pictures.clone()
        });
        if pictures.as_ref().is_some_and(|p| p.read(cx).busy()) {
            self.store
                .update(cx, |store, cx| store.set_busy(wait, true, cx));
            self.waiting.insert(wait, (text, done));
            return;
        }
        let text = text.read(cx);
        let saved = pictures
            .as_ref()
            .map(|p| p.read(cx).saved())
            .unwrap_or_default();
        if text.trim().is_empty() && saved.is_empty() {
            let note = ui::say(cx, Str::ComposerNothingSaved).to_owned();
            self.notes.insert(wait, note);
            self.store
                .update(cx, |store, cx| store.set_busy(wait, false, cx));
            cx.notify();
            return;
        }
        let (ids, paths): (Vec<u32>, Vec<String>) = saved.into_iter().unzip();
        let task = self.store.update(cx, |store, cx| {
            store.answer_wait(WaitId(wait), text, paths, cx)
        });
        let done: Done = Box::new(move |cx| {
            if let Some(done) = done {
                done(cx);
            }
            if let Some(pictures) = pictures {
                pictures.update(cx, |pictures, cx| pictures.forget(&ids, cx));
            }
        });
        self.run(wait, Some(done), task, cx);
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

    pub(crate) fn choose_nth(
        &mut self,
        wait: &ScreenWait,
        nth: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = wait.wait.id.0;
        if let Some(prompt) = &wait.wait.prompt {
            if prompt.options.get(nth).is_none_or(|option| option.writes) {
                return false;
            }
            self.shown.insert(id, prompt.clone());
            self.pick(id, nth, cx);
            return true;
        }
        let answerable = wait.wait.answerable && matches!(wait.tone, WaitTone::Ask);
        let Some(choice) = wait.wait.choices.get(nth).filter(|_| answerable) else {
            return false;
        };
        self.reply(id, choice.key.to_string(), cx);
        true
    }

    fn reply(&mut self, wait: u32, text: String, cx: &mut Context<Self>) {
        if self.store.read(cx).is_busy(wait) {
            return;
        }
        self.send(wait, Text::Typed(text), None, cx);
    }

    pub(crate) fn dismiss(&mut self, wait: u32, cx: &mut Context<Self>) {
        let id = WaitId(wait);
        let task = self
            .store
            .update(cx, |store, cx| store.dismiss_wait(id, cx));
        self.run(wait, None, task, cx);
    }
}

fn hint(shown: bool, index: usize) -> Option<String> {
    (shown && index < 9).then(|| format!("Alt+{}", index + 1))
}

fn scaled(
    answers: &Entity<Answers>,
    id: u32,
    slot: Slot,
    tone: WaitTone,
    z: f32,
    cx: &mut App,
) -> Option<Entity<Field>> {
    let field = answers.read(cx).fields.get(&(id, slot))?.field.clone();
    let color = entry::tone_of(tone, &ui::colors(cx));
    field.update(cx, |field, cx| {
        field.set_scale(z, cx);
        field.set_tone(color, cx);
    });
    Some(field)
}

pub(crate) fn entry(
    answers: &Entity<Answers>,
    node: NodeId,
    wait: &ScreenWait,
    z: f32,
    find: Option<&Find>,
    cx: &mut App,
) -> Div {
    let body = wait_body(answers, node, wait, z, find, cx);
    framed(answers, wait, z, find, body, None, cx)
}

pub(crate) struct Strip {
    pub repo: String,
    pub node: String,
    pub counts: String,
    pub bulk: Option<AnyElement>,
    pub note: Option<String>,
}

pub(crate) fn card(
    answers: &Entity<Answers>,
    node: NodeId,
    wait: &ScreenWait,
    z: f32,
    strip: Strip,
    hints: bool,
    cx: &mut App,
) -> Div {
    let mut body = wait_content(answers, wait, z, None, hints, cx);
    if !wait.wait.answerable && wait.wait.prompt.is_none() {
        body = body.child(terminal_link(answers, node, wait.wait.id.0, z, cx));
    }
    framed(answers, wait, z, None, body, Some(strip), cx)
}

fn framed(
    answers: &Entity<Answers>,
    wait: &ScreenWait,
    z: f32,
    find: Option<&Find>,
    body: Div,
    strip: Option<Strip>,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let id = wait.wait.id.0;
    let done = matches!(wait.tone, WaitTone::Done);
    let busy = answers.read(cx).store.read(cx).is_busy(id);
    let dismiss = {
        let answers = answers.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
            answers.update(cx, |answers, cx| answers.dismiss(id, cx));
        }
    };
    let refused = answers.read(cx).notes.get(&id).cloned();
    let close = if done {
        ui::button_at(("close", id as usize), z, cx)
            .flex_none()
            .child(ui::say(cx, Str::InboxConfirm))
    } else {
        entry::ghost_close("close", id as usize, z, &c).tooltip(tip(refused
            .clone()
            .unwrap_or_else(|| ui::say(cx, Str::InboxDismiss).to_owned())))
    }
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(dismiss);
    let carded = strip.is_some();
    let top = div()
        .flex()
        .items_center()
        .gap(px(GAP * z))
        .min_w_0()
        .child(entry::mark(wait.tone, z, &c))
        .children(
            (!carded || wait.wait.kind != WaitKind::Question)
                .then(|| entry::clip(ui::say(cx, kind_label(wait.wait.kind)), z, &c).flex_none()),
        )
        .children((wait.wait.kind == WaitKind::Question).then(|| {
            find::chip(
                find,
                id,
                entry::qid(wait.wait.id.to_string(), wait.tone, 20.0, z, &c),
            )
        }));
    let (top, note) = match strip {
        Some(strip) => {
            let small = |text: String| {
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(px(SMALL * z))
                    .text_color(c.ink_3)
                    .child(text)
            };
            let left = top.child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.0 * z))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_color(c.ink_3)
                            .child(format!("{} \u{203a}", strip.repo)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(strip.node),
                    )
                    .child(small(ui::clock(wait.wait.since, "%H:%M")).ml(px(6.0 * z))),
            );
            let right = div()
                .flex()
                .items_center()
                .gap(px(GAP * z))
                .flex_none()
                .ml_auto()
                .child(small(strip.counts))
                .children(strip.bulk)
                .child(close);
            let wrapped = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(GAP * z))
                .gap_y(px(4.0 * z))
                .child(left)
                .child(right);
            (wrapped, strip.note)
        }
        None => (
            top.child(entry::published(wait.wait.since, z, cx))
                .child(close),
            None,
        ),
    };
    let mut entry = div()
        .flex()
        .flex_col()
        .gap(px(GAP * z))
        .opacity(if busy { 0.6 } else { 1.0 })
        .child(top);
    if let Some(text) = &wait.wait.instruction {
        entry = entry.child(
            div()
                .text_size(px(SMALL * z))
                .text_color(c.ink)
                .child(find::instruction(find, cx, text)),
        );
    }
    if let Some(text) = note {
        entry = entry.child(
            div()
                .text_size(px(SMALL * z))
                .text_color(c.ink_3)
                .child(text),
        );
    }
    entry = entry.child(body);
    let viewer = {
        let state = answers.read(cx);
        (state.zoomed == Some(id) && state.viewer.read(cx).is_open()).then(|| state.viewer.clone())
    };
    entry = entry.children(viewer);
    if let Some(note) = refused {
        entry = entry.child(div().text_size(px(SMALL * z)).text_color(c.err).child(note));
    }
    entry
}

fn wait_content(
    answers: &Entity<Answers>,
    wait: &ScreenWait,
    z: f32,
    find: Option<&Find>,
    hints: bool,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let id = wait.wait.id.0;
    let done = matches!(wait.tone, WaitTone::Done);
    let group = format!("wait-{id}");
    let mut body = div()
        .group(group.clone())
        .relative()
        .flex()
        .flex_col()
        .gap(px(GAP * z));
    let shots: Vec<(usize, Arc<Image>)> = (0..wait.wait.images.len())
        .filter_map(|index| Some((index, answers.read(cx).images.get(&(id, index))?.clone())))
        .collect();
    if !shots.is_empty() {
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0 * z))
                .pt(px(4.0 * z))
                .children(shots.into_iter().map(|(index, image)| {
                    let owner = answers.clone();
                    let opened = image.clone();
                    let name = wait.wait.images[index]
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    div()
                        .id(("shot", id as usize * 256 + index % 256))
                        .cursor_pointer()
                        .tooltip(tip(ui::say_with(
                            cx,
                            Str::InboxZoomImage,
                            &[("name", &name)],
                        )))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let opened = opened.clone();
                            owner.update(cx, |answers, cx| answers.zoom(id, opened, window, cx))
                        })
                        .child(
                            img(image)
                                .w(px(SHOT_WIDTH * z))
                                .h(px(SHOT_HEIGHT * z))
                                .object_fit(ObjectFit::Cover)
                                .rounded(px(6.0 * z))
                                .border_1()
                                .border_color(c.line_strong)
                                .bg(c.panel),
                        )
                })),
        );
    }
    if wait.wait.prompt.is_none() {
        answers.update(cx, |answers, _| {
            answers.shown.remove(&id);
        });
    }
    if let Some(prompt) = &wait.wait.prompt {
        answers.update(cx, |answers, _| {
            if answers.shown.get(&id) != Some(prompt) {
                answers.shown.insert(id, prompt.clone());
            }
        });
        body = body.child(prompt_body(answers, wait, prompt, z, find, hints, cx));
    } else if !wait.wait.text.is_empty() || !wait.wait.blocks.is_empty() || wait.wait.doc.is_some()
    {
        let owner = answers.clone();
        body = body.child(div().text_size(px(ui::TEXT * z)).child(blocks::render(
            ("text", id as usize),
            &wait.wait.blocks,
            &wait.wait.text,
            wait.wait.doc.as_ref(),
            entry::tone_of(wait.tone, &c),
            find,
            ui::TEXT * z,
            cx,
            move |target, cx| owner.update(cx, |answers, cx| answers.follow(id, target, cx)),
        )));
    }
    if wait.wait.answerable && !done && !wait.wait.choices.is_empty() {
        body = body.child(div().flex().flex_wrap().gap(px(GAP * z)).children(
            wait.wait.choices.iter().enumerate().map(|(index, choice)| {
                let key = choice.key;
                let owner = answers.clone();
                let look = if choice.recommended {
                    Look::Recommended
                } else {
                    Look::Plain
                };
                let button = div()
                    .id(("choice", id as usize * 256 + key as usize % 256))
                    .cursor_pointer()
                    .hover({
                        let hover = c.surface_hover;
                        move |style| style.bg(hover)
                    });
                entry::choice_face(
                    button,
                    Choice {
                        key: key.to_string(),
                        hint: hint(hints, index),
                        label: &choice.label,
                        recommended: choice.recommended,
                        look,
                        tone: wait.tone,
                    },
                    z,
                    cx,
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    owner.update(cx, |answers, cx| answers.reply(id, key.to_string(), cx))
                })
            }),
        ));
    }
    body
}

fn wait_body(
    answers: &Entity<Answers>,
    node: NodeId,
    wait: &ScreenWait,
    z: f32,
    find: Option<&Find>,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let id = wait.wait.id.0;
    let group = format!("wait-{id}");
    let qid = if wait.wait.kind == WaitKind::Question {
        wait.wait.id.to_string()
    } else {
        ui::say(cx, kind_label(wait.wait.kind)).to_owned()
    };
    let mut body = wait_content(answers, wait, z, find, false, cx);
    if wait.wait.answerable {
        let pictures = answers
            .read(cx)
            .attached
            .get(&id)
            .map(|a| a.pictures.clone());
        if let Some(pictures) = &pictures {
            let owner = answers.clone();
            body = body.children(pictures::chips(
                pictures,
                move |picture, window, cx| {
                    owner.update(cx, |answers, cx| answers.zoom(id, picture, window, cx))
                },
                "answer-chip",
                id,
                z,
                cx,
            ));
        }
        let field = scaled(answers, id, Slot::Answer, wait.tone, z, cx);
        if let Some(field) = field {
            body = body.child(send_row(
                answers,
                id,
                Slot::Answer,
                wait.tone,
                &field,
                z,
                cx,
            ));
        }
        if let Some(pictures) = pictures {
            let cover = div()
                .absolute()
                .inset(px(-4.0 * z))
                .invisible()
                .group_drag_over::<ExternalPaths>(group, |style| style.visible())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(2.0 * z))
                .rounded(px(8.0 * z))
                .border(px(2.0 * z))
                .border_dashed()
                .border_color(entry::tone_of(wait.tone, &c))
                .bg(mix(entry::tone_of(wait.tone, &c), 0.1, c.surface))
                .text_color(c.ink)
                .text_size(px(ui::TEXT * z))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon(
                    Lucide::ImagePlus,
                    20.0 * z,
                    entry::tone_of(wait.tone, &c),
                ))
                .child(ui::say_with(cx, Str::InboxDropImage, &[("id", &qid)]))
                .child(
                    div()
                        .text_size(px(SMALL * z))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(c.ink_2)
                        .child(ui::say_with(
                            cx,
                            Str::InboxDropLimit,
                            &[("limit", &pictures::LIMIT_MEGABYTES.to_string())],
                        )),
                );
            body = body
                .child(cover)
                .on_drop(move |paths: &ExternalPaths, _, cx| {
                    pictures.update(cx, |pictures, cx| {
                        pictures.take_paths(paths.paths().to_vec(), cx)
                    })
                });
        }
    } else if wait.wait.prompt.is_none() {
        body = body.child(terminal_link(answers, node, id, z, cx));
    }
    body
}

fn terminal_link(
    answers: &Entity<Answers>,
    node: NodeId,
    id: u32,
    z: f32,
    cx: &mut App,
) -> Stateful<Div> {
    let c = ui::colors(cx);
    let store = answers.read(cx).store.clone();
    let hover = c.surface_hover;
    let ink = c.ink;
    div()
        .id(("terminal", id as usize))
        .self_start()
        .flex()
        .items_center()
        .gap(px(GAP * z))
        .max_w_full()
        .min_w_0()
        .h(px(28.0 * z))
        .ml(px(-8.0 * z))
        .px(px(8.0 * z))
        .rounded(px(6.0 * z))
        .text_size(px(SMALL * z))
        .text_color(c.ink_2)
        .underline()
        .cursor_pointer()
        .hover(move |style| style.bg(hover).text_color(ink))
        .child(ui::say(cx, Str::InboxToTerminal))
        .child(icon(Lucide::ArrowRight, 12.0 * z, c.ink_2))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| {
            store.update(cx, |store, cx| store.go(node.0, cx));
        })
}

fn send_row(
    answers: &Entity<Answers>,
    id: u32,
    slot: Slot,
    tone: WaitTone,
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
    let empty = {
        let state = answers.read(cx);
        let bare = state
            .attached
            .get(&id)
            .is_none_or(|a| a.pictures.read(cx).is_empty());
        slot == Slot::Answer && field.read(cx).is_blank() && bare
    };
    let busy = answers.read(cx).store.read(cx).is_busy(id);
    let off = empty || (slot != Slot::Answer && busy);
    let color = entry::tone_of(tone, &c);
    let mut send = div()
        .id(("send", id as usize * 256 + slot_id))
        .flex_none()
        .size(px(28.0 * z))
        .rounded(px(6.0 * z))
        .flex()
        .items_center()
        .justify_center()
        .bg(color)
        .child(icon(Lucide::ArrowUp, 14.0 * z, entry::tone_ink(tone, &c)));
    if slot != Slot::Answer {
        send = send.tooltip(tip(ui::say(cx, Str::InboxTypeSend).to_owned()));
    }
    let send = if off {
        send.opacity(0.45)
    } else {
        send.cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| owner.update(cx, |answers, cx| answers.submit(id, slot, cx)))
    };
    div()
        .flex()
        .items_end()
        .gap(px(GAP * z))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().flex_1().min_w_0().child(field.clone()))
        .child(send)
}

fn prompt_body(
    answers: &Entity<Answers>,
    wait: &ScreenWait,
    prompt: &Prompt,
    z: f32,
    find: Option<&Find>,
    hints: bool,
    cx: &mut App,
) -> AnyElement {
    let (id, tone) = (wait.wait.id.0, wait.tone);
    let c = ui::colors(cx);
    let busy = answers.read(cx).store.read(cx).is_busy(id);
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(GAP * z))
        .child(entry::prompt_box(
            find::line(find, "", &prompt.body.join("\n"), ""),
            z,
            &c,
        ));
    let options =
        div()
            .flex()
            .flex_col()
            .gap(px(GAP * z))
            .children(prompt.options.iter().enumerate().map(|(index, option)| {
                let label = key_label(&option.key, index);
                if option.writes {
                    return entry::choice_face(
                        div(),
                        Choice {
                            key: label,
                            hint: None,
                            label: &option.label,
                            recommended: false,
                            look: Look::Plain,
                            tone,
                        },
                        z,
                        cx,
                    )
                    .into_any_element();
                }
                let owner = answers.clone();
                let button = div()
                    .id(("option", id as usize * 256 + index))
                    .cursor_pointer()
                    .when(busy, |button| button.opacity(0.6))
                    .hover({
                        let hover = c.surface_hover;
                        move |style| style.bg(hover)
                    });
                entry::choice_face(
                    button,
                    Choice {
                        key: label,
                        hint: hint(hints, index),
                        label: &option.label,
                        recommended: index == 0,
                        look: if index == 0 {
                            Look::Recommended
                        } else {
                            Look::Plain
                        },
                        tone,
                    },
                    z,
                    cx,
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    owner.update(cx, |answers, cx| answers.pick(id, index, cx))
                })
                .into_any_element()
            }));
    if !prompt.options.is_empty() {
        body = body.child(options);
    }
    for (index, option) in prompt.options.iter().enumerate() {
        let field = option
            .writes
            .then(|| scaled(answers, id, Slot::Option(index), tone, z, cx))
            .flatten();
        if let Some(field) = field {
            body = body.child(send_row(
                answers,
                id,
                Slot::Option(index),
                tone,
                &field,
                z,
                cx,
            ));
        }
    }
    let field = scaled(answers, id, Slot::Field, tone, z, cx);
    if let Some(field) = field {
        body = body.child(send_row(answers, id, Slot::Field, tone, &field, z, cx));
    }
    body.into_any_element()
}
