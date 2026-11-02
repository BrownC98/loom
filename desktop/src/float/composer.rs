use std::cell::Cell;
use std::rc::Rc;

use api::ids::NodeId;
use api::settings::{FloatPlace, FloatSize};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, BoxShadow, Context, Div, Entity, ExternalPaths,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement,
    Pixels, Point, Render, ScrollEvent, Size, Stateful, StatefulInteractiveElement, Styled,
    Subscription, Window, canvas, div, point, px,
};

use super::picker::{Picker, PickerEvent, picker};
use super::place::{self, Grab, MARGIN};
use crate::canvas::minimap;
use crate::store::Store;
use crate::strings::Str;
use crate::theme::mix;
use crate::views::dock::ui;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};
use crate::views::inputs::Inputs;
use crate::views::pictures::{self, Pictures, PicturesEvent};
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
const PICK_WIDTH: f32 = 380.0;

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
    sending: Option<u32>,
    files: Vec<Filed>,
    viewer: Entity<Viewer>,
    chevron: Rc<Cell<Bounds<Pixels>>>,
    listing: Option<(Entity<Picker>, Subscription)>,
    subscriptions: Vec<Subscription>,
}

pub fn composer(
    store: Entity<Store>,
    registry: Entity<Inputs>,
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
                    if let Some(node) = composer.target(cx) {
                        let registry = composer.inputs.clone();
                        registry.update(cx, |inputs, cx| inputs.focus(node, window, cx));
                    }
                }
                FieldEvent::Changed | FieldEvent::Step(_) => {}
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
            files: Vec::new(),
            viewer: cx.new(Viewer::new),
            chevron: Rc::new(Cell::new(Bounds::default())),
            listing: None,
            subscriptions: vec![typed, heard],
        };
        composer
            .subscriptions
            .push(cx.observe(&store, |composer: &mut Composer, _, cx| composer.sync(cx)));
        composer
            .subscriptions
            .push(cx.observe(&registry, |_, _, cx| cx.notify()));
        composer.sync(cx);
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

struct Look {
    size: FloatSize,
    saved: Option<FloatPlace>,
    corner: f32,
}

impl Composer {
    fn target(&self, cx: &App) -> Option<u32> {
        let store = self.store.read(cx);
        let id = store.selected()?;
        store
            .snapshot()?
            .nodes
            .iter()
            .any(|n| n.id.0 == id)
            .then_some(id)
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let place = self.store.read(cx).settings().composer_place;
        if place != self.seen_place {
            self.seen_place = place;
            self.kept = None;
        }
        if let Some(node) = self.target(cx) {
            self.pictures
                .update(cx, |pictures, _| pictures.aim(NodeId(node)));
        }
        cx.notify();
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
        let Some(node) = self.target(cx).map(NodeId) else {
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
                self.sending = None;
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
        let Some(target) = self.target(cx) else {
            return;
        };
        self.sending = Some(target);
        cx.notify();
        self.flush(cx);
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.sending else {
            return;
        };
        self.move_files(target, cx);
        self.pictures
            .update(cx, |pictures, cx| pictures.retarget(NodeId(target), cx));
        let pictures = self.pictures.clone();
        if self.filing > 0 || pictures.read(cx).busy() {
            return;
        }
        self.sending = None;
        let body = self.field.read(cx).text().trim_end().to_owned();
        let saved = pictures.read(cx).saved();
        let attached = !pictures.read(cx).is_empty();
        if body.trim().is_empty() && saved.is_empty() {
            if attached {
                let text = ui::say(cx, Str::ComposerNothingSaved).to_owned();
                self.refuse(text, cx);
            }
            return;
        }
        let paths: Vec<String> = saved.iter().map(|(_, path)| quote(path)).collect();
        let text = [body, paths.join(" ")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let state = self
            .inputs
            .update(cx, |inputs, cx| inputs.deliver(target, &text, cx));
        match state {
            Some(InputState::Open) => {
                let ids: Vec<u32> = saved.iter().map(|(id, _)| *id).collect();
                pictures.update(cx, |pictures, cx| pictures.forget(&ids, cx));
                self.files.clear();
                self.field.update(cx, |field, cx| field.clear(cx));
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

    fn toggle_list(&mut self, node: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.listing.take().is_some() {
            cx.notify();
            return;
        }
        let list = picker(self.store.clone(), node, self.chevron.clone(), window, cx);
        let sub = cx.subscribe_in(
            &list,
            window,
            move |composer, _, event: &PickerEvent, window, cx| {
                composer.listing = None;
                match event {
                    PickerEvent::Pick(id) => composer.pick(node, *id, window, cx),
                    PickerEvent::Escape => composer.focus_field(window, cx),
                    PickerEvent::Close => {}
                }
                cx.notify();
            },
        );
        self.listing = Some((list, sub));
        cx.notify();
    }

    fn focus_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.field.read(cx).handle();
        window.focus(&handle, cx);
    }

    fn pick(&mut self, current: u32, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.show_node(id, cx);
            store.select(id, cx);
        });
        if id == current {
            self.focus_field(window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FOCUS_DELAY).await;
            let _ = this.update_in(cx, |composer, window, cx| composer.focus_field(window, cx));
        })
        .detach();
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
        let store = self.store.read(cx);
        let node = store.selected().and_then(|id| {
            let view = store.snapshot()?.nodes.iter().find(|n| n.id.0 == id)?;
            Some((id, view.name.clone()))
        });
        let shown = node.filter(|(id, _)| {
            !store.settings().composer_off && self.inputs.read(cx).state(*id, cx).is_some()
        });
        let Some((node, name)) = shown else {
            self.grab = None;
            self.at = None;
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
        let face = self.face(node, &name, home, z, at, window, cx);
        let wrapper = div()
            .relative()
            .occlude()
            .w(px(width))
            .child(place::measure(self.boxed.clone(), owner))
            .child(face);
        let mut float = div().absolute().left(at.x).top(at.y);
        if !measured {
            float = float.invisible();
        }
        let pop = self.listing.as_ref().map(|(list, _)| {
            let (above, room) = place::pop_space(f32::from(at.y), boxed, pane, None, z);
            let wide = (PICK_WIDTH * z).min((f32::from(pane.width) - 2.0 * MARGIN).max(0.0));
            let left = (f32::from(at.x)
                .min(f32::from(pane.width) - MARGIN - wide)
                .max(MARGIN))
                - f32::from(at.x);
            list.update(cx, |list, cx| list.place(z, wide, room, above, cx));
            let gap = px(place::POP_GAP * z);
            let anchored = div().absolute().left(px(left)).child(list.clone());
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
    #[allow(clippy::too_many_arguments)]
    fn face(
        &self,
        node: u32,
        name: &str,
        home: bool,
        z: f32,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.field.update(cx, |field, cx| field.set_scale(z, cx));
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
        let view = ComposerView {
            name,
            home,
            z,
            focused,
            sendable,
            listing: self.listing.is_some(),
            filing: (self.filing > 0).then(|| {
                ui::say_with(
                    cx,
                    Str::ComposerFiling,
                    &[("count", &self.filing.to_string())],
                )
            }),
            note: self.note.clone(),
        };
        let field = self.field.clone().into_any_element();
        let live = Live {
            cx,
            node,
            at,
            anchor: self.chevron.clone(),
            sendable,
        };
        face(&view, images, field, &live, cx)
    }
}

pub struct ComposerView<'a> {
    pub name: &'a str,
    pub home: bool,
    pub z: f32,
    pub focused: bool,
    pub sendable: bool,
    pub listing: bool,
    pub filing: Option<String>,
    pub note: Option<String>,
}

pub trait ComposerWire {
    fn shell(&self, el: Div) -> Div {
        el
    }
    fn head(&self, el: Div) -> Div {
        el
    }
    fn name(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn chevron(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn reset(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn image(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn file(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
    fn send(&self, el: Stateful<Div>) -> Stateful<Div> {
        el
    }
}

pub fn face<W: ComposerWire>(
    view: &ComposerView,
    images: Option<AnyElement>,
    field: AnyElement,
    wire: &W,
    cx: &App,
) -> Div {
    let c = ui::colors(cx);
    let z = view.z;
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
    let chevron = div()
        .id("composer-chevron")
        .flex_none()
        .size(px(18.0 * z))
        .rounded(px(4.0 * z))
        .flex()
        .items_center()
        .justify_center()
        .text_color(c.ink_2)
        .when(view.listing, |d| d.bg(c.surface_hover).text_color(c.ink))
        .child(icon(
            if view.listing {
                Lucide::ChevronUp
            } else {
                Lucide::ChevronDown
            },
            14.0 * z,
            if view.listing { c.ink } else { c.ink_2 },
        ));
    let target = div()
        .flex()
        .items_center()
        .gap(px(4.0 * z))
        .min_w_0()
        .overflow_hidden()
        .child(icon(Lucide::SquareTerminal, 14.0 * z, c.ink_3))
        .child(
            wire.name(
                div()
                    .id("composer-name")
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c.ink)
                    .child(view.name.to_owned()),
            ),
        )
        .child(wire.chevron(chevron))
        .child(ui::say(cx, Str::ComposerTo));
    let head = wire.head(
        div()
            .flex()
            .items_center()
            .gap(px(6.0 * z))
            .min_w_0()
            .text_size(px(12.0 * z))
            .text_color(c.ink_3)
            .child(target)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .child(ui::say(cx, Str::ComposerKeys)),
            )
            .children(
                (!view.home).then(|| wire.reset(tool("composer-reset", 22.0).child("\u{21ba}"))),
            ),
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
    let line = div()
        .flex()
        .items_end()
        .gap(px(4.0 * z))
        .child(div().flex_1().min_w_0().child(field))
        .child(wire.image(tool("composer-image", BUTTON).child(icon(
            Lucide::Image,
            16.0 * z,
            c.ink_2,
        ))))
        .child(wire.file(tool("composer-file", BUTTON).child(icon(
            Lucide::Paperclip,
            16.0 * z,
            c.ink_2,
        ))))
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
                    .bg(shade(c.accent, view.sendable, c.panel))
                    .child(icon(
                        Lucide::ArrowUp,
                        16.0 * z,
                        shade(c.accent_ink, view.sendable, c.panel),
                    )),
            ),
        );
    wire.shell(
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
            .child(head)
            .children(images)
            .children(filing)
            .children(note)
            .child(line),
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
    node: u32,
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

impl Live<'_, '_> {
    fn pick(&self, el: Stateful<Div>, label: Str) -> Stateful<Div> {
        self.tool(el, label)
            .on_click(self.cx.listener(move |composer, _, _, cx| {
                composer
                    .pictures
                    .update(cx, |pictures, cx| pictures.pick_files(cx));
            }))
    }
}

impl ComposerWire for Live<'_, '_> {
    fn shell(&self, el: Div) -> Div {
        el.on_mouse_down(MouseButton::Left, place::stop)
            .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
            .on_drop(
                self.cx
                    .listener(move |composer, paths: &ExternalPaths, _, cx| {
                        composer.pictures.update(cx, |pictures, cx| {
                            pictures.take_paths(paths.paths().to_vec(), cx)
                        });
                    }),
            )
    }

    fn head(&self, el: Div) -> Div {
        let at = self.at;
        el.cursor_grab().on_mouse_down(
            MouseButton::Left,
            self.cx
                .listener(move |composer, e: &MouseDownEvent, _, cx| composer.grab(e, at, cx)),
        )
    }

    fn name(&self, el: Stateful<Div>) -> Stateful<Div> {
        let node = self.node;
        el.cursor_pointer()
            .hover(|s| s.underline())
            .tooltip(tip(ui::say(self.cx, Str::ComposerShowNode).to_owned()))
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_click(self.cx.listener(move |composer, _, _, cx| {
                composer
                    .store
                    .update(cx, |store, cx| store.show_node(node, cx));
            }))
    }

    fn chevron(&self, el: Stateful<Div>) -> Stateful<Div> {
        let (node, anchor) = (self.node, self.anchor.clone());
        let c = ui::colors(self.cx);
        el.cursor_pointer()
            .hover(move |s| s.bg(c.surface_hover).text_color(c.ink))
            .tooltip(tip(ui::say(self.cx, Str::ComposerPickNode).to_owned()))
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_click(
                self.cx.listener(move |composer, _, window, cx| {
                    composer.toggle_list(node, window, cx)
                }),
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

    fn image(&self, el: Stateful<Div>) -> Stateful<Div> {
        self.pick(el, Str::ComposerAttachImage)
    }

    fn file(&self, el: Stateful<Div>) -> Stateful<Div> {
        self.pick(el, Str::ComposerAttachFile)
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
