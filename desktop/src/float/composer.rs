use std::cell::Cell;
use std::rc::Rc;

use api::graph::waits::WaitTone;
use api::ids::{NodeId, WaitId};
use api::settings::{FloatPlace, FloatSize};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, BoxShadow, Context, Div, Entity, ExternalPaths,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollEvent, Size, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Window, canvas, deferred, div, point, px,
};

use super::aim::{self, Aim, Chosen, Kind, Stop, Streamed};
use super::picker::{Picker, PickerEvent, picker};
use super::place::{self, Grab, MARGIN};
use crate::canvas::minimap;
use crate::store::Store;
use crate::strings::Str;
use crate::theme::mix;
use crate::views::dock::answers::{self, Answers, Strip};
use crate::views::dock::entry;
use crate::views::dock::questions::kind_label;
use crate::views::dock::ui;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};
use crate::views::inputs::Inputs;
use crate::views::pictures::{self, Pictures, PicturesEvent, Shelf};
use crate::views::scrollbar::Bar;
use crate::views::terminal::InputState;
use crate::views::tooltip::tip;
use crate::views::viewer::Viewer;

const HOME: FloatPlace = FloatPlace { x: 0.5, y: 1.0 };
const WIDTH: f32 = 600.0;
const SHADOW_BLUR: f32 = 14.0;
const SHADOW_DROP: f32 = 4.0;
const BUTTON: f32 = 30.0;
const DISABLED_ALPHA: f32 = 0.4;
const FOCUS_DELAY: std::time::Duration = std::time::Duration::from_millis(120);
const LEAD: f32 = 22.0;
const CARD_ROOM: f32 = 280.0;
const STREAM_FLOOR: f32 = 52.0;
const STREAM_TIME: &str = "%H:%M";
const KEYS_FONT: f32 = 11.0;
const KEYS_GAP: f32 = 6.0;
const KEYS_ROOM: f32 = 480.0;

struct Filed {
    node: NodeId,
    path: String,
    name: String,
    bytes: Vec<u8>,
    moving: bool,
}

pub struct Composer {
    store: Entity<Store>,
    inputs: Entity<Inputs>,
    pane: Rc<Cell<Size<Pixels>>>,
    boxed: Rc<Cell<Size<Pixels>>>,
    grab: Option<Grab>,
    at: Option<Point<Pixels>>,
    kept: Option<Option<FloatPlace>>,
    seen_place: Option<FloatPlace>,
    field: Entity<Field>,
    pictures: Entity<Pictures>,
    note: Option<String>,
    filing: usize,
    sending: Option<Sending>,
    aborted: bool,
    files: Vec<Filed>,
    viewer: Entity<Viewer>,
    answers: Entity<Answers>,
    chosen: Option<Chosen>,
    pinned: Option<Stop>,
    seen_selected: Option<u32>,
    seen_node: Option<u32>,
    last: Option<Chosen>,
    stream_bar: Bar,
    stream_box: Rc<Cell<Size<Pixels>>>,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    listing: Option<(Entity<Picker>, Subscription)>,
    bulk_open: Cell<Option<u32>>,
    subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
struct Sending {
    node: u32,
    kind: Kind,
}

struct Aimed {
    node: u32,
    name: String,
    stream: Vec<Streamed>,
    aim: Aim,
    kind: Kind,
    lost: bool,
}

impl Aimed {
    fn shown(&self) -> Option<&Streamed> {
        match self.aim {
            Aim::Alert(id) => self.stream.iter().find(|s| s.wait.wait.id == id),
            Aim::Prompt => self.stream.iter().find(|s| s.node == self.node),
        }
    }

    fn asks(&self) -> usize {
        self.stream
            .iter()
            .filter(|s| crate::views::status::asking(&s.wait))
            .count()
    }

    fn dones(&self) -> usize {
        self.stream
            .iter()
            .filter(|s| matches!(s.wait.tone, WaitTone::Done))
            .count()
    }
}

pub fn composer(
    store: Entity<Store>,
    registry: Entity<Inputs>,
    shelf: Entity<Shelf>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Composer> {
    cx.new(|cx| {
        let seen_place = store.read(cx).settings().composer_place;
        let field = field::boxed_label(Str::ComposerPlaceholder, false, true, cx);
        let pictures = cx.new(|_| Pictures::new(store.clone(), NodeId(0)));
        let attach = pictures.clone();
        field.update(cx, |field, _| {
            field.on_files(move |entries, _, cx| {
                attach.update(cx, |pictures, cx| pictures.take_clipboard(entries, cx));
            });
        });
        let typed = cx.subscribe_in(
            &field,
            window,
            |composer: &mut Composer, _, event: &FieldEvent, window, cx| match event {
                FieldEvent::Enter => composer.send(cx),
                FieldEvent::Escape => {
                    if composer.bulk_open.take().is_some() {
                        cx.notify();
                    } else if let Some(node) = composer.node(cx) {
                        let registry = composer.inputs.clone();
                        registry.update(cx, |inputs, cx| inputs.focus(node, window, cx));
                    }
                }
                FieldEvent::Changed => composer.repin(cx),
                FieldEvent::Step(_) => {}
            },
        );
        let heard = cx.subscribe_in(
            &pictures,
            window,
            |composer: &mut Composer, _, event: &PicturesEvent, window, cx| match event {
                PicturesEvent::Settled => composer.flush(cx),
                PicturesEvent::Refused(text) => composer.refuse(text.clone(), cx),
                PicturesEvent::Other(name, bytes) => {
                    composer.take_file(name.clone(), bytes.clone(), window, cx)
                }
            },
        );
        let shared = pictures.clone();
        let mut composer = Composer {
            store: store.clone(),
            inputs: registry.clone(),
            pane: Rc::new(Cell::new(Size::default())),
            boxed: Rc::new(Cell::new(Size::default())),
            grab: None,
            at: None,
            kept: None,
            seen_place,
            field,
            pictures,
            note: None,
            filing: 0,
            sending: None,
            aborted: false,
            files: Vec::new(),
            viewer: cx.new(Viewer::new),
            answers: cx.new(|cx| {
                let mut answers = Answers::new(store.clone(), shelf, cx);
                answers.share(shared);
                answers
            }),
            chosen: None,
            pinned: None,
            seen_selected: None,
            seen_node: None,
            last: None,
            stream_bar: Bar::new(),
            stream_box: Rc::new(Cell::new(Size::default())),
            anchor: Rc::new(Cell::new(Bounds::default())),
            listing: None,
            bulk_open: Cell::new(None),
            subscriptions: vec![typed, heard],
        };
        composer.subscriptions.push(cx.observe_in(
            &store,
            window,
            |composer: &mut Composer, _, window, cx| composer.sync(window, cx),
        ));
        composer
            .subscriptions
            .push(cx.observe(&registry, |_, _, cx| cx.notify()));
        composer
            .subscriptions
            .push(cx.observe(&composer.answers, |_, _, cx| cx.notify()));
        composer.sync(window, cx);
        composer
    })
}

fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn shadow(color: gpui::Rgba) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.into(),
        offset: point(px(0.0), px(SHADOW_DROP)),
        blur_radius: px(SHADOW_BLUR),
        spread_radius: px(0.0),
        inset: false,
    }]
}

#[derive(Clone, Copy)]
struct Spot {
    home: bool,
    z: f32,
    width: f32,
    at: Point<Pixels>,
}

struct Look {
    size: FloatSize,
    saved: Option<FloatPlace>,
    corner: f32,
}

impl Composer {
    fn aimed(&self, cx: &App) -> Option<Aimed> {
        let store = self.store.read(cx);
        let snapshot = store.snapshot()?;
        let stream = aim::stream_of(snapshot);
        let exists = |id: &u32| snapshot.nodes.iter().any(|n| n.id.0 == *id);
        let held = self.pinned.filter(|(held, _)| exists(held));
        let chosen = self.chosen.filter(|c| exists(&c.stop.0));
        let fallback = store.selected().filter(exists);
        let (node, aim) = aim::resolve(held, chosen, &stream, fallback)?;
        let name = snapshot.nodes.iter().find(|n| n.id.0 == node)?.name.clone();
        let lost = (self.pinned.is_some() && held.is_none())
            || matches!(aim, Aim::Alert(id) if !stream.iter().any(|s| s.wait.wait.id == id));
        let kind = aim::kind_of(aim, &stream);
        Some(Aimed {
            node,
            name,
            stream,
            aim,
            kind,
            lost,
        })
    }

    fn stream(&self, cx: &App) -> Vec<Streamed> {
        self.store
            .read(cx)
            .snapshot()
            .map(aim::stream_of)
            .unwrap_or_default()
    }

    fn choose(&mut self, stop: Stop, cx: &mut Context<Self>) {
        self.chosen = Some(aim::chosen(stop, &self.stream(cx)));
    }

    fn repin(&mut self, cx: &mut Context<Self>) {
        if self.field.read(cx).is_blank() {
            self.pinned = None;
        } else if self.pinned.is_none() {
            self.pinned = self.aimed(cx).map(|a| (a.node, a.aim));
        }
        cx.notify();
    }

    fn reaim(&mut self, cx: &mut Context<Self>) {
        self.pinned = None;
        self.repin(cx);
    }

    fn node(&self, cx: &App) -> Option<u32> {
        self.aimed(cx).map(|a| a.node)
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.store.read(cx).selected();
        if selected != self.seen_selected {
            self.seen_selected = selected;
            let gone = self.pinned.is_some_and(|(held, _)| {
                !self
                    .store
                    .read(cx)
                    .snapshot()
                    .is_some_and(|s| s.nodes.iter().any(|n| n.id.0 == held))
            });
            if let Some(node) = selected
                && Some(node) != self.node(cx)
            {
                let stream = self.stream(cx);
                let first = stream.iter().find(|s| s.node == node).map(Streamed::stop);
                self.choose(first.unwrap_or((node, Aim::Prompt)), cx);
            }
            if !gone {
                self.reaim(cx);
            }
        }
        self.settle(cx);
        self.follow(window, cx);
        let place = self.store.read(cx).settings().composer_place;
        if place != self.seen_place {
            self.seen_place = place;
            self.kept = None;
        }
        if let Some(node) = self.node(cx) {
            self.pictures
                .update(cx, |pictures, _| pictures.aim(NodeId(node)));
        }
        cx.notify();
    }

    fn settle(&mut self, cx: &mut Context<Self>) {
        let Some(aimed) = self.aimed(cx) else { return };
        if let Some(last) = self.last
            && let Aim::Alert(id) = last.stop.1
            && self.pinned.is_none()
            && !aimed.stream.iter().any(|s| s.wait.wait.id == id)
            && !self
                .answers
                .update(cx, |answers, _| answers.take_acted(id.0))
        {
            let same: Vec<&Streamed> = aimed
                .stream
                .iter()
                .filter(|s| s.node == last.stop.0)
                .collect();
            let next = last
                .place
                .and_then(|at| same.iter().find(|s| s.place() > at))
                .or(same.first());
            let stop = next.map_or((last.stop.0, Aim::Prompt), |s| s.stop());
            self.choose(stop, cx);
        }
        let now = self.aimed(cx);
        self.last = now.and_then(|aimed| match aimed.aim {
            Aim::Alert(id) if aimed.stream.iter().any(|s| s.wait.wait.id == id) => {
                Some(aim::chosen((aimed.node, aimed.aim), &aimed.stream))
            }
            _ => None,
        });
    }

    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(aimed) = self.aimed(cx) else { return };
        let moved = self.seen_node.is_some_and(|node| node != aimed.node);
        self.seen_node = Some(aimed.node);
        if moved
            && matches!(aimed.aim, Aim::Alert(_))
            && self.store.read(cx).selected() != Some(aimed.node)
        {
            let typing = self.field.read(cx).focused(window);
            self.store.update(cx, |store, cx| store.go(aimed.node, cx));
            if typing {
                self.focus_later(window, cx);
            }
        }
    }

    fn focus_later(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FOCUS_DELAY).await;
            let _ = this.update_in(cx, |composer, window, cx| composer.focus_field(window, cx));
        })
        .detach();
    }

    fn look(&self, cx: &Context<Self>) -> Look {
        let store = self.store.read(cx);
        let settings = store.settings();
        Look {
            size: settings.composer_size,
            saved: self.kept.unwrap_or(settings.composer_place),
            corner: if store.zoomed() {
                0.0
            } else if settings.minimap_folded {
                minimap::CORNER_FOLDED
            } else {
                minimap::CORNER
            },
        }
    }

    fn full_width(&self, look: &Look) -> f32 {
        let room = (f32::from(self.pane.get().width) - 2.0 * MARGIN).max(0.0);
        (WIDTH * place::zoom(look.size)).min(room)
    }

    fn put(&mut self, patch: serde_json::Value, cx: &mut Context<Self>) {
        self.store
            .update(cx, |store, cx| store.save_settings(patch, cx));
    }

    fn grab(&mut self, event: &MouseDownEvent, origin: Point<Pixels>, cx: &mut Context<Self>) {
        self.grab = Some(Grab::new(event.position, origin));
        cx.notify();
    }

    fn slide(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(grab) = &mut self.grab else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.grab = None;
            return;
        }
        let Some(origin) = grab.drag(event.position) else {
            return;
        };
        let look = self.look(cx);
        let boxed = Size {
            width: px(self.full_width(&look)),
            height: self.boxed.get().height,
        };
        let pane = self.pane.get();
        self.at = Some(place::place_of(
            place::place_from(origin, pane, boxed),
            pane,
            boxed,
        ));
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        let Some(grab) = self.grab.take() else { return };
        if !grab.moved() {
            return;
        }
        if let Some(at) = self.at.take() {
            let look = self.look(cx);
            let boxed = Size {
                width: px(self.full_width(&look)),
                height: self.boxed.get().height,
            };
            let dropped = place::place_from(at, self.pane.get(), boxed);
            self.kept = Some(Some(dropped));
            self.put(place::patch("composerPlace", Some(dropped)), cx);
        }
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.kept = Some(None);
        self.put(place::patch("composerPlace", None), cx);
        cx.notify();
    }

    fn refuse(&mut self, text: String, cx: &mut Context<Self>) {
        self.note = Some(text);
        cx.notify();
    }

    fn take_file(
        &mut self,
        name: String,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.node(cx).map(NodeId) else {
            return;
        };
        self.note = None;
        self.filing += 1;
        let task = self.store.update(cx, |store, cx| {
            store.save_paste(
                node,
                name.clone(),
                "application/octet-stream",
                bytes.clone(),
                cx,
            )
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let saved = task.await.map(|path| Filed {
                node,
                path,
                name,
                bytes,
                moving: false,
            });
            let _ = this.update_in(cx, |composer, window, cx| {
                composer.uploaded(saved, window, cx)
            });
        })
        .detach();
    }

    fn uploaded(
        &mut self,
        saved: Result<Filed, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filing = self.filing.saturating_sub(1);
        match saved {
            Ok(filed) => {
                let token = quote(&filed.path);
                self.files.push(filed);
                self.field
                    .update(cx, |field, cx| field.insert_token(&token, window, cx));
            }
            Err(error) => self.note = Some(error),
        }
        cx.notify();
        self.flush(cx);
    }

    fn move_files(&mut self, target: u32, cx: &mut Context<Self>) {
        let body = self.field.read(cx).text().to_owned();
        self.files
            .retain(|filed| body.contains(&quote(&filed.path)));
        let node = NodeId(target);
        for filed in self
            .files
            .iter_mut()
            .filter(|filed| filed.node != node && !filed.moving)
        {
            filed.moving = true;
            self.filing += 1;
            let old = filed.path.clone();
            let (name, bytes) = (filed.name.clone(), filed.bytes.clone());
            let task = self.store.update(cx, |store, cx| {
                store.save_paste(node, name, "application/octet-stream", bytes, cx)
            });
            cx.spawn(async move |this, cx| {
                let saved = task.await;
                let _ = this.update(cx, |composer, cx| composer.moved(&old, node, saved, cx));
            })
            .detach();
        }
    }

    fn moved(
        &mut self,
        old: &str,
        node: NodeId,
        saved: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        self.filing = self.filing.saturating_sub(1);
        let Some(filed) = self.files.iter_mut().find(|filed| filed.path == old) else {
            self.flush(cx);
            return;
        };
        filed.moving = false;
        match saved {
            Ok(path) => {
                let text = self
                    .field
                    .read(cx)
                    .text()
                    .replace(&quote(old), &quote(&path));
                self.field
                    .update(cx, |field, cx| field.set_text_at_end(&text, cx));
                filed.path = path;
                filed.node = node;
            }
            Err(error) => {
                self.aborted = true;
                self.note = Some(error);
            }
        }
        cx.notify();
        self.flush(cx);
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        self.note = None;
        if self.sending.is_some() {
            return;
        }
        let Some(aimed) = self.aimed(cx) else {
            return;
        };
        if aimed.lost {
            let text = ui::say(cx, Str::ComposerTargetClosed).to_owned();
            self.reaim(cx);
            self.refuse(text, cx);
            return;
        }
        let sending = Sending {
            node: aimed.node,
            kind: aimed.kind,
        };
        self.set_sending(Some(sending), cx);
        self.flush(cx);
    }

    fn set_sending(&mut self, sending: Option<Sending>, cx: &mut Context<Self>) {
        self.sending = sending;
        let locked = sending.is_some();
        self.field
            .update(cx, |field, cx| field.set_locked(locked, cx));
        self.pictures
            .update(cx, |pictures, cx| pictures.set_locked(locked, cx));
        cx.notify();
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        let Some(Sending { node, kind }) = self.sending else {
            return;
        };
        if !self.aborted {
            self.move_files(node, cx);
            self.pictures
                .update(cx, |pictures, cx| pictures.retarget(NodeId(node), cx));
        }
        let pictures = self.pictures.clone();
        if self.filing > 0 || pictures.read(cx).busy() {
            return;
        }
        let aborted = std::mem::take(&mut self.aborted);
        let body = self.field.read(cx).text().trim_end().to_owned();
        let saved = pictures.read(cx).saved();
        let attached = !pictures.read(cx).is_empty();
        if aborted || (body.trim().is_empty() && saved.is_empty()) {
            self.set_sending(None, cx);
            if !aborted && attached {
                let text = ui::say(cx, Str::ComposerNothingSaved).to_owned();
                self.refuse(text, cx);
            }
            return;
        }
        match kind {
            Kind::Prompt => {
                self.set_sending(None, cx);
                self.type_in(node, body, saved, cx);
            }
            Kind::Answer(wait) | Kind::Continue(wait) => self.answer(wait, body, saved, cx),
        }
    }

    fn type_in(
        &mut self,
        node: u32,
        body: String,
        saved: Vec<(u32, String)>,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<String> = saved.iter().map(|(_, path)| quote(path)).collect();
        let text = [body, paths.join(" ")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let state = self
            .inputs
            .update(cx, |inputs, cx| inputs.deliver(node, &text, cx));
        match state {
            Some(InputState::Open) => {
                let ids: Vec<u32> = saved.iter().map(|(id, _)| *id).collect();
                self.sent(&ids, cx);
            }
            Some(InputState::Offline) => {
                let text = ui::say(cx, Str::ComposerOffline).to_owned();
                self.refuse(text, cx);
            }
            Some(InputState::Gone) | None => {
                let text = ui::say(cx, Str::ComposerGone).to_owned();
                self.refuse(text, cx);
            }
        }
        cx.notify();
    }

    fn answer(
        &mut self,
        wait: WaitId,
        body: String,
        saved: Vec<(u32, String)>,
        cx: &mut Context<Self>,
    ) {
        let (ids, paths): (Vec<u32>, Vec<String>) = saved.into_iter().unzip();
        self.answers
            .update(cx, |answers, _| answers.mark_acted(wait.0));
        let task = self
            .store
            .update(cx, |store, cx| store.answer_wait(wait, body, paths, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |composer, cx| {
                composer.set_sending(None, cx);
                match result {
                    Ok(()) => composer.sent(&ids, cx),
                    Err(error) => {
                        composer
                            .answers
                            .update(cx, |answers, _| answers.unmark_acted(wait.0));
                        composer.refuse(error, cx)
                    }
                }
            });
        })
        .detach();
    }

    fn sent(&mut self, ids: &[u32], cx: &mut Context<Self>) {
        self.pictures
            .update(cx, |pictures, cx| pictures.forget(ids, cx));
        self.files.clear();
        self.field.update(cx, |field, cx| field.clear(cx));
        cx.notify();
    }

    fn toggle_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.listing.take().is_some() {
            cx.notify();
            return;
        }
        let Some(aimed) = self.aimed(cx) else {
            return;
        };
        let list = picker(
            self.store.clone(),
            (NodeId(aimed.node), aimed.aim),
            self.anchor.clone(),
            window,
            cx,
        );
        let sub = cx.subscribe_in(
            &list,
            window,
            move |composer, _, event: &PickerEvent, window, cx| {
                composer.listing = None;
                match event {
                    PickerEvent::Pick(node, aim) => composer.pick(*node, *aim, window, cx),
                    PickerEvent::Escape => composer.focus_field(window, cx),
                    PickerEvent::Close => {}
                }
                cx.notify();
            },
        );
        self.listing = Some((list, sub));
        cx.notify();
    }

    fn cycle(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(aimed) = self.aimed(cx) else {
            return;
        };
        let next = aim::cycle((aimed.node, aimed.aim), &aimed.stream, by);
        self.choose(next, cx);
        self.reaim(cx);
        self.follow(window, cx);
    }

    fn confirm_all(&mut self, cx: &mut Context<Self>) {
        self.bulk_open.set(None);
        let ids: Vec<u32> = self
            .stream(cx)
            .iter()
            .filter(|s| matches!(s.wait.tone, WaitTone::Done))
            .map(|s| s.wait.wait.id.0)
            .collect();
        self.answers.update(cx, |answers, cx| {
            for id in ids {
                answers.dismiss(id, cx);
            }
        });
    }

    fn answer_nth(&mut self, nth: usize, cx: &mut Context<Self>) -> bool {
        let Some(aimed) = self.aimed(cx) else {
            return false;
        };
        let Aim::Alert(id) = aimed.aim else {
            return false;
        };
        let Some(wait) = aimed.stream.iter().find(|s| s.wait.wait.id == id) else {
            return false;
        };
        self.answers
            .update(cx, |answers, cx| answers.choose_nth(&wait.wait, nth, cx))
    }

    fn keys(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let modifiers = keys.modifiers;
        if !modifiers.alt || modifiers.control || modifiers.platform || modifiers.shift {
            return;
        }
        match keys.key.as_str() {
            "up" if self.listing.is_none() => self.cycle(-1, window, cx),
            "down" if self.listing.is_none() => self.cycle(1, window, cx),
            "l" => self.toggle_list(window, cx),
            digit if digit.len() == 1 && ("1"..="9").contains(&digit) => {
                let nth = usize::from(digit.as_bytes()[0] - b'1');
                if self.listing.is_some() || !self.answer_nth(nth, cx) {
                    return;
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn focus_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.field.read(cx).handle();
        window.focus(&handle, cx);
    }

    fn pick(&mut self, node: NodeId, aim: Aim, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.node(cx);
        self.choose((node.0, aim), cx);
        self.reaim(cx);
        self.store.update(cx, |store, cx| {
            store.show_node(node.0, cx);
            store.select(node.0, cx);
        });
        self.follow(window, cx);
        if current == Some(node.0) {
            self.focus_field(window, cx);
            return;
        }
        self.focus_later(window, cx);
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.entity_id();
        let root = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(place::measure(self.pane.clone(), owner));
        let off = self.store.read(cx).settings().composer_off;
        let composing = self.field.read(cx).composing();
        let shown = self.aimed(cx).filter(|a| {
            composing
                || !a.stream.is_empty()
                || (!off && self.inputs.read(cx).state(a.node, cx).is_some())
        });
        let Some(aimed) = shown else {
            self.grab = None;
            self.at = None;
            self.listing = None;
            self.store
                .update(cx, |store, cx| store.set_composer(None, cx));
            return root;
        };
        let look = self.look(cx);
        let z = place::zoom(look.size);
        let (pane, boxed) = (self.pane.get(), self.boxed.get());
        let measured = boxed.width > px(0.0) && pane.width > px(0.0);
        let home = look.saved.is_none();
        let beside = home && self.at.is_none();
        let full = self.full_width(&look);
        let width = if beside {
            full.min(f32::from(pane.width) - look.corner - 2.0 * MARGIN)
                .max(0.0)
        } else {
            full
        };
        let placed = place::place_of(
            look.saved.unwrap_or(HOME),
            pane,
            Size {
                width: px(width),
                height: boxed.height,
            },
        );
        let at = self.at.unwrap_or_else(|| {
            if beside {
                let limit = f32::from(pane.width) - look.corner - MARGIN - width;
                point(placed.x.min(px(limit)), placed.y)
            } else {
                placed
            }
        });
        if self.at.is_none() {
            let shown = measured.then(|| (gpui::Bounds::new(at, boxed), home));
            self.store
                .update(cx, |store, cx| store.set_composer(shown, cx));
        }
        let spot = Spot { home, z, width, at };
        let face = self.face(&aimed, &spot, window, cx);
        let wrapper = div()
            .relative()
            .occlude()
            .w(px(width))
            .child(place::measure(self.boxed.clone(), owner))
            .child(face);
        let mut float = div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .on_key_down(cx.listener(Self::keys));
        if !measured {
            float = float.invisible();
        }
        let pop = self.listing.as_ref().map(|(list, _)| {
            let (above, room) = place::pop_space(f32::from(at.y), boxed, pane, None, z);
            list.update(cx, |list, cx| list.place(z, width, room, above, cx));
            let gap = px(place::POP_GAP * z);
            let anchored = div().absolute().left_0().child(list.clone());
            if above {
                anchored.bottom(boxed.height + gap)
            } else {
                anchored.top(boxed.height + gap)
            }
        });
        let follow = self
            .grab
            .is_some()
            .then(|| place::follow(cx.entity(), Composer::slide, Composer::release));
        let viewer = self.viewer.read(cx).is_open().then(|| self.viewer.clone());
        root.children(follow)
            .child(float.child(wrapper).children(pop))
            .children(viewer)
    }
}

impl Composer {
    fn bulk_for(
        &self,
        shown: &Streamed,
        done: usize,
        z: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let wait = shown.wait.wait.id.0;
        let shows = done > 1 && matches!(shown.wait.tone, WaitTone::Done);
        if !shows || self.bulk_open.get().is_some_and(|open| open != wait) {
            self.bulk_open.set(None);
        }
        shows.then(|| self.bulk(wait, done, z, cx))
    }

    fn bulk(&self, wait: u32, done: usize, z: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = ui::colors(cx);
        let open = self.bulk_open.get() == Some(wait);
        let arrow = ui::button_at("composer-confirm-more", z, cx)
            .flex_none()
            .px(px(4.0 * z))
            .ml(px(-1.0))
            .rounded_l(px(0.0))
            .when(open, |arrow| arrow.bg(c.surface_hover))
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_click(cx.listener(move |composer, _, _, cx| {
                composer.bulk_open.set((!open).then_some(wait));
                cx.notify();
            }))
            .child(icon(Lucide::ChevronDown, 14.0 * z, c.ink));
        let menu = open.then(|| {
            let text = ui::say_with(cx, Str::ComposerConfirmAll, &[("count", &done.to_string())]);
            let hover = c.surface_hover;
            let item = div()
                .id("composer-confirm-all")
                .flex()
                .items_center()
                .gap(px(8.0 * z))
                .h(px(30.0 * z))
                .px(px(8.0 * z))
                .rounded(px(6.0 * z))
                .whitespace_nowrap()
                .text_size(px(13.0 * z))
                .text_color(c.ink)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_mouse_down(MouseButton::Left, place::stop)
                .on_click(cx.listener(|composer, _, _, cx| composer.confirm_all(cx)))
                .child(icon(Lucide::CircleCheck, 14.0 * z, c.ok))
                .child(text);
            let panel = div()
                .absolute()
                .top(px(30.0 * z))
                .right_0()
                .p(px(6.0 * z))
                .rounded(px(10.0 * z))
                .bg(c.surface)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|composer, _, _, cx| {
                    composer.bulk_open.set(None);
                    cx.notify();
                }))
                .child(item);
            deferred(panel).with_priority(2)
        });
        div()
            .relative()
            .flex_none()
            .flex()
            .child(arrow)
            .children(menu)
            .into_any_element()
    }

    fn card(&self, aimed: &Aimed, z: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = ui::colors(cx);
        let Some(shown) = aimed.shown() else {
            self.bulk_open.set(None);
            return None;
        };
        let ask = aimed.asks();
        let done = aimed.dones();
        let counts = ui::say_with(
            cx,
            Str::ComposerAlertTotal,
            &[
                ("total", &aimed.stream.len().to_string()),
                ("ask", &ask.to_string()),
                ("done", &done.to_string()),
            ],
        );
        let strip = Strip {
            repo: shown.repo.clone(),
            node: shown.name.clone(),
            counts,
            bulk: self.bulk_for(shown, done, z, cx),
            note: (matches!(aimed.aim, Aim::Prompt) && !matches!(shown.wait.tone, WaitTone::Done))
                .then(|| ui::say(cx, Str::ComposerNotAnswer).to_owned()),
        };
        let hints = matches!(aimed.aim, Aim::Alert(_));
        let body = answers::card(
            &self.answers,
            NodeId(shown.node),
            &shown.wait,
            z,
            strip,
            hints,
            cx,
        );
        let node = shown.node;
        Some(
            div()
                .id("composer-card")
                .cursor_pointer()
                .on_click(cx.listener(move |composer, _, _, cx| {
                    composer.store.update(cx, |store, cx| store.go(node, cx));
                }))
                .flex()
                .flex_col()
                .max_h(px(CARD_ROOM * z))
                .px(px(10.0 * z))
                .py(px(8.0 * z))
                .rounded(px(10.0 * z))
                .border_1()
                .border_color(entry::tone_of(shown.wait.tone, &c))
                .bg(c.surface)
                .text_size(px(13.0 * z))
                .child(body)
                .into_any_element(),
        )
    }

    fn stream_list(&self, aimed: &Aimed, z: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = ui::colors(cx);
        let current = aimed.shown().map(|s| s.wait.wait.id);
        let rows: Vec<_> = aimed
            .stream
            .iter()
            .filter(|s| Some(s.wait.wait.id) != current)
            .collect();
        if rows.is_empty() {
            self.stream_box.set(Size::default());
            return None;
        }
        let rest = self.boxed.get().height - self.stream_box.get().height;
        let room = self.pane.get().height - px(2.0 * MARGIN) - rest;
        let mut list = div()
            .id("composer-stream")
            .flex()
            .flex_col()
            .max_h(room.max(px(STREAM_FLOOR * z)))
            .overflow_y_scroll()
            .track_scroll(self.stream_bar.handle());
        for (index, row) in rows.into_iter().enumerate() {
            let id = row.wait.wait.id;
            let node = row.node;
            let hover = c.surface_hover;
            let chip = if row.wait.wait.kind == api::graph::waits::WaitKind::Question {
                id.to_string()
            } else {
                ui::say(cx, kind_label(row.wait.wait.kind)).to_owned()
            };
            list = list.child(
                div()
                    .id(("composer-stream-row", index))
                    .flex()
                    .items_center()
                    .gap(px(8.0 * z))
                    .min_h(px(26.0 * z))
                    .px(px(8.0 * z))
                    .rounded(px(6.0 * z))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |composer, _, window, cx| {
                        composer.pick(NodeId(node), Aim::Alert(id), window, cx)
                    }))
                    .child(entry::mark(row.wait.tone, z, &c))
                    .child(entry::qid(chip, row.wait.tone, 18.0, z, &c))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(140.0 * z))
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.ink_2)
                            .child(row.wait.summary.replace('\n', " ")),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0 * z))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(c.ink_3)
                            .child(ui::clock(row.wait.wait.since, STREAM_TIME)),
                    ),
            );
        }
        let list = list.child(place::measure(self.stream_box.clone(), cx.entity_id()));
        Some(self.stream_bar.reserved(list).min_h_0().into_any_element())
    }

    fn face(
        &self,
        aimed: &Aimed,
        spot: &Spot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Spot { home, z, width, at } = *spot;
        let c = ui::colors(cx);
        let (tone, _) = aimed.kind.paint(&c);
        let label = match aimed.kind {
            Kind::Answer(_) => Str::InboxAnswerPlaceholder,
            Kind::Continue(_) => Str::InboxFollowUpPlaceholder,
            Kind::Prompt => Str::ComposerPlaceholder,
        };
        let keys = (width >= KEYS_ROOM * z).then(|| ui::say(cx, Str::ComposerKeys).to_owned());
        let trail = keys.as_ref().map_or(0.0, |text| {
            let style = window.text_style();
            let run = style.to_run(text.len());
            let line = window.text_system().shape_line(
                text.clone().into(),
                px(KEYS_FONT * z),
                &[run],
                None,
            );
            f32::from(line.width) / z + KEYS_GAP
        });
        self.field.update(cx, |field, cx| {
            field.set_scale(z, cx);
            field.set_tone(tone, cx);
            field.set_rest(tone, cx);
            field.set_inset(LEAD, trail, cx);
            field.set_label(label, cx);
        });
        let focused = self.field.read(cx).focused(window);
        let ready = !self.field.read(cx).is_blank()
            || !self.pictures.read(cx).is_empty()
            || self.filing > 0;
        let sendable = ready && self.sending.is_none();
        let viewer = self.viewer.clone();
        let images = pictures::chips(
            &self.pictures,
            move |picture, window, cx| {
                viewer.update(cx, |viewer, cx| viewer.open(picture, window, cx))
            },
            "composer-chip",
            0,
            z,
            cx,
        )
        .map(IntoElement::into_any_element);
        if aimed.stream.is_empty() {
            self.bulk_open.set(None);
        }
        let alerts = (!aimed.stream.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.0 * z))
                .children(self.stream_list(aimed, z, cx))
                .children(self.card(aimed, z, cx))
                .into_any_element()
        });
        let view = ComposerView {
            name: &aimed.name,
            kind: aimed.kind,
            home,
            z,
            focused,
            sendable,
            listing: self.listing.is_some(),
            keys,
            filing: (self.filing > 0).then(|| {
                ui::say_with(
                    cx,
                    Str::ComposerFiling,
                    &[("count", &self.filing.to_string())],
                )
            }),
            note: self.note.clone().or_else(|| {
                aimed
                    .lost
                    .then(|| ui::say(cx, Str::ComposerTargetClosed).to_owned())
            }),
        };
        let field = self.field.clone().into_any_element();
        let live = Live {
            cx,
            at,
            anchor: self.anchor.clone(),
            sendable,
        };
        face(&view, alerts, images, field, &live, cx)
    }
}

pub struct ComposerView<'a> {
    pub name: &'a str,
    pub kind: Kind,
    pub home: bool,
    pub z: f32,
    pub focused: bool,
    pub sendable: bool,
    pub listing: bool,
    pub keys: Option<String>,
    pub filing: Option<String>,
    pub note: Option<String>,
}

pub trait ComposerWire {
    fn shell(&self, el: Div) -> Div {
        el
    }
    fn drag(&self, el: Div) -> Div {
        el
    }
    fn target(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn reset(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn attach(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn send(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
}

pub fn face<W: ComposerWire>(
    view: &ComposerView,
    card: Option<AnyElement>,
    images: Option<AnyElement>,
    field: AnyElement,
    wire: &W,
    cx: &App,
) -> Div {
    let c = ui::colors(cx);
    let z = view.z;
    let (tone, tone_ink) = view.kind.paint(&c);
    let tool = |id: &'static str, size: f32| {
        div()
            .id(id)
            .flex_none()
            .size(px(size * z))
            .rounded(px(6.0 * z))
            .flex()
            .items_center()
            .justify_center()
            .text_color(c.ink_2)
    };
    let target = wire.target(
        div()
            .id("composer-target")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.0 * z))
            .max_w(px(220.0 * z))
            .h(px(BUTTON * z))
            .pl(px(10.0 * z))
            .pr(px(6.0 * z))
            .rounded(px(8.0 * z))
            .border_1()
            .border_color(tone)
            .bg(tone)
            .text_color(tone_ink)
            .text_size(px(12.5 * z))
            .font_weight(FontWeight::SEMIBOLD)
            .child(icon(view.kind.glyph(), 14.0 * z, tone_ink))
            .child(div().min_w_0().truncate().child(ui::say_with(
                cx,
                Str::ComposerToNode,
                &[("name", view.name)],
            )))
            .child(icon(
                if view.listing {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronUp
                },
                14.0 * z,
                tone_ink,
            )),
    );
    let filing = view.filing.clone().map(|text| {
        div()
            .text_size(px(12.0 * z))
            .text_color(c.ink_3)
            .child(text)
    });
    let note = view
        .note
        .clone()
        .map(|text| div().text_size(px(12.0 * z)).text_color(c.err).child(text));
    let attach = wire.attach(
        tool("composer-attach", 22.0)
            .absolute()
            .left(px(4.0 * z))
            .top(px(4.0 * z))
            .child(icon(Lucide::Paperclip, 16.0 * z, c.ink_2)),
    );
    let keys = view.keys.clone().map(|text| {
        div()
            .absolute()
            .right(px(8.0 * z))
            .bottom(px(5.0 * z))
            .h(px(18.0 * z))
            .flex()
            .items_center()
            .text_size(px(KEYS_FONT * z))
            .text_color(c.ink_3)
            .whitespace_nowrap()
            .child(text)
    });
    let line = div()
        .flex()
        .items_end()
        .gap(px(6.0 * z))
        .child(target)
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .on_mouse_down(MouseButton::Left, place::stop)
                .child(field)
                .child(attach)
                .children(keys),
        )
        .child(
            wire.send(
                div()
                    .id("composer-send")
                    .flex_none()
                    .size(px(BUTTON * z))
                    .rounded(px(8.0 * z))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(shade(tone, view.sendable, c.panel))
                    .child(icon(
                        Lucide::ArrowUp,
                        16.0 * z,
                        shade(tone_ink, view.sendable, c.panel),
                    )),
            ),
        )
        .children((!view.home).then(|| {
            wire.reset(tool("composer-reset", 22.0).h(px(BUTTON * z)).child(icon(
                Lucide::RotateCcw,
                14.0 * z,
                c.ink_3,
            )))
        }));
    let still = |el: AnyElement| {
        div()
            .cursor_default()
            .on_mouse_down(MouseButton::Left, place::stop)
            .child(el)
    };
    wire.shell(
        wire.drag(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0 * z))
                .pt(px(8.0 * z))
                .pr(px(8.0 * z))
                .pb(px(8.0 * z))
                .pl(px(10.0 * z))
                .rounded(px(14.0 * z))
                .border_1()
                .border_color(if view.focused {
                    c.accent
                } else {
                    c.line_strong
                })
                .bg(c.panel)
                .text_color(c.ink)
                .text_size(px(13.0 * z))
                .shadow(shadow(c.shadow))
                .children(card.map(still))
                .children(images.map(still))
                .children(filing)
                .children(note)
                .child(line),
        ),
    )
}

fn shade(color: gpui::Rgba, live: bool, under: gpui::Rgba) -> gpui::Rgba {
    if live {
        color
    } else {
        mix(color, DISABLED_ALPHA, under)
    }
}

struct Live<'a, 'b> {
    cx: &'a Context<'b, Composer>,
    at: Point<Pixels>,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    sendable: bool,
}

impl Live<'_, '_> {
    fn tool(&self, el: Stateful<Div>, label: Str) -> Stateful<Div> {
        let c = ui::colors(self.cx);
        el.cursor_pointer()
            .hover(move |s| s.bg(c.surface_hover).text_color(c.ink))
            .on_mouse_down(MouseButton::Left, place::stop)
            .tooltip(tip(ui::say(self.cx, label).to_owned()))
    }
}

impl ComposerWire for Live<'_, '_> {
    fn shell(&self, el: Div) -> Div {
        el.on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
            .on_drop(
                self.cx
                    .listener(move |composer, paths: &ExternalPaths, _, cx| {
                        composer.pictures.update(cx, |pictures, cx| {
                            pictures.take_paths(paths.paths().to_vec(), cx)
                        });
                    }),
            )
    }

    fn drag(&self, el: Div) -> Div {
        let at = self.at;
        el.cursor_grab().on_mouse_down(
            MouseButton::Left,
            self.cx
                .listener(move |composer, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    composer.grab(e, at, cx);
                }),
        )
    }

    fn target(&self, el: Stateful<Div>) -> Stateful<Div> {
        let anchor = self.anchor.clone();
        el.cursor_pointer()
            .tooltip(tip(ui::say(self.cx, Str::ComposerPickTarget).to_owned()))
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_click(
                self.cx
                    .listener(move |composer, _, window, cx| composer.toggle_list(window, cx)),
            )
            .child(
                canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| ())
                    .absolute()
                    .size_full(),
            )
    }

    fn reset(&self, el: Stateful<Div>) -> Stateful<Div> {
        self.tool(el, Str::ComposerResetPlace)
            .on_click(self.cx.listener(|composer, _, _, cx| composer.reset(cx)))
    }

    fn attach(&self, el: Stateful<Div>) -> Stateful<Div> {
        self.tool(el, Str::ComposerAttachFile)
            .on_click(self.cx.listener(move |composer, _, _, cx| {
                composer
                    .pictures
                    .update(cx, |pictures, cx| pictures.pick_files(cx));
            }))
    }

    fn send(&self, el: Stateful<Div>) -> Stateful<Div> {
        let el = el
            .tooltip(tip(ui::say(self.cx, Str::ComposerSend).to_owned()))
            .on_mouse_down(MouseButton::Left, place::stop);
        if self.sendable {
            el.cursor_pointer()
                .on_click(self.cx.listener(|composer, _, _, cx| composer.send(cx)))
        } else {
            el
        }
    }
}
