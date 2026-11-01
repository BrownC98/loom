use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use api::ids::NodeId;
use api::settings::{FloatPlace, FloatSize};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, BoxShadow, Context, Entity, ExternalPaths, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render,
    ScrollEvent, Size, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div,
    point, px,
};

use super::inputs::{Inputs, inputs};
use super::picker::{Picker, PickerEvent, picker};
use super::place::{self, Grab, MARGIN};
use crate::canvas::minimap;
use crate::store::Store;
use crate::strings::Str;
use crate::views::dock::ui;
use crate::views::field::{self, Field, FieldEvent};
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

struct Draft {
    field: Entity<Field>,
    pictures: Entity<Pictures>,
    note: Option<String>,
    filing: usize,
    sending: bool,
    _subs: [Subscription; 2],
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
    drafts: HashMap<u32, Draft>,
    viewer: Entity<Viewer>,
    chevron: Rc<Cell<Bounds<Pixels>>>,
    listing: Option<(Entity<Picker>, Subscription)>,
    subscriptions: Vec<Subscription>,
}

pub fn composer(store: Entity<Store>, window: &mut Window, cx: &mut App) -> Entity<Composer> {
    cx.new(|cx| {
        let registry = inputs(cx);
        let seen_place = store.read(cx).settings().composer_place;
        let mut composer = Composer {
            store: store.clone(),
            inputs: registry.clone(),
            pane: Rc::new(Cell::new(Size::default())),
            boxed: Rc::new(Cell::new(Size::default())),
            grab: None,
            at: None,
            kept: None,
            seen_place,
            drafts: HashMap::new(),
            viewer: cx.new(Viewer::new),
            chevron: Rc::new(Cell::new(Bounds::default())),
            listing: None,
            subscriptions: Vec::new(),
        };
        composer.subscriptions.push(cx.observe_in(
            &store,
            window,
            |composer: &mut Composer, _, window, cx| composer.sync(window, cx),
        ));
        composer
            .subscriptions
            .push(cx.observe(&registry, |_, _, cx| cx.notify()));
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

struct Look {
    size: FloatSize,
    saved: Option<FloatPlace>,
    corner: f32,
}

impl Composer {
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let place = store.settings().composer_place;
        let live: HashSet<u32> = store
            .snapshot()
            .map(|s| s.nodes.iter().map(|n| n.id.0).collect())
            .unwrap_or_default();
        let selected = store.selected().filter(|id| live.contains(id));
        if place != self.seen_place {
            self.seen_place = place;
            self.kept = None;
        }
        self.drafts.retain(|id, _| live.contains(id));
        if let Some(node) = selected {
            self.ensure(node, window, cx);
        }
        cx.notify();
    }

    fn ensure(&mut self, node: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.drafts.contains_key(&node) {
            return;
        }
        let field = field::boxed(ui::say(cx, Str::ComposerPlaceholder), false, true, cx);
        let pictures = cx.new(|_| Pictures::new(self.store.clone(), NodeId(node)));
        let attach = pictures.clone();
        field.update(cx, |field, _| {
            field.on_files(move |entries, _, cx| {
                attach.update(cx, |pictures, cx| pictures.take_clipboard(entries, cx));
            });
        });
        let sub = cx.subscribe_in(
            &field,
            window,
            move |composer, _, event: &FieldEvent, window, cx| match event {
                FieldEvent::Enter => composer.send(node, window, cx),
                FieldEvent::Escape => {
                    let registry = composer.inputs.clone();
                    registry.update(cx, |inputs, cx| inputs.focus(node, window, cx));
                }
                FieldEvent::Changed | FieldEvent::Step(_) => {}
            },
        );
        let heard = cx.subscribe_in(
            &pictures,
            window,
            move |composer, _, event: &PicturesEvent, window, cx| match event {
                PicturesEvent::Settled => composer.flush(node, cx),
                PicturesEvent::Refused(text) => composer.refuse(node, text.clone(), cx),
                PicturesEvent::Other(name, bytes) => {
                    composer.take_file(node, name.clone(), bytes.clone(), window, cx)
                }
            },
        );
        self.drafts.insert(
            node,
            Draft {
                field,
                pictures,
                note: None,
                filing: 0,
                sending: false,
                _subs: [sub, heard],
            },
        );
    }

    fn look(&self, cx: &Context<Self>) -> Look {
        let store = self.store.read(cx);
        let settings = store.settings();
        Look {
            size: settings.composer_size,
            saved: self.kept.unwrap_or(settings.composer_place),
            corner: if settings.minimap_folded || store.zoomed() {
                0.0
            } else {
                minimap::WIDTH + MARGIN
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

    fn refuse(&mut self, node: u32, text: String, cx: &mut Context<Self>) {
        if let Some(draft) = self.drafts.get_mut(&node) {
            draft.note = Some(text);
            cx.notify();
        }
    }

    fn take_file(
        &mut self,
        node: u32,
        name: String,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.drafts.get_mut(&node) else {
            return;
        };
        draft.note = None;
        draft.filing += 1;
        let task = self.store.update(cx, |store, cx| {
            store.save_paste(NodeId(node), name, "application/octet-stream", bytes, cx)
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let saved = task.await;
            let _ = this.update_in(cx, |composer, window, cx| {
                composer.uploaded(node, saved, window, cx)
            });
        })
        .detach();
    }

    fn uploaded(
        &mut self,
        node: u32,
        saved: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.drafts.get_mut(&node) else {
            return;
        };
        draft.filing = draft.filing.saturating_sub(1);
        match saved {
            Ok(path) => {
                let field = draft.field.clone();
                field.update(cx, |field, cx| {
                    field.insert_token(&quote(&path), window, cx)
                });
            }
            Err(error) => draft.note = Some(error),
        }
        cx.notify();
        self.flush(node, cx);
    }

    fn send(&mut self, node: u32, _: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.drafts.get_mut(&node) else {
            return;
        };
        draft.note = None;
        if draft.sending {
            return;
        }
        draft.sending = true;
        cx.notify();
        self.flush(node, cx);
    }

    fn flush(&mut self, node: u32, cx: &mut Context<Self>) {
        let Some(draft) = self.drafts.get_mut(&node) else {
            return;
        };
        let pictures = draft.pictures.clone();
        if !draft.sending || draft.filing > 0 || pictures.read(cx).busy() {
            return;
        }
        draft.sending = false;
        let body = draft.field.read(cx).text().trim_end().to_owned();
        let saved = pictures.read(cx).saved();
        let attached = !pictures.read(cx).is_empty();
        if body.trim().is_empty() && saved.is_empty() {
            if attached {
                let text = ui::say(cx, Str::ComposerNothingSaved).to_owned();
                self.refuse(node, text, cx);
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
            .update(cx, |inputs, cx| inputs.deliver(node, &text, cx));
        match state {
            Some(InputState::Open) => {
                if let Some(draft) = self.drafts.get_mut(&node) {
                    let ids: Vec<u32> = saved.iter().map(|(id, _)| *id).collect();
                    pictures.update(cx, |pictures, cx| pictures.forget(&ids, cx));
                    let field = draft.field.clone();
                    field.update(cx, |field, cx| field.clear(cx));
                }
            }
            Some(InputState::Offline) => {
                let text = ui::say(cx, Str::ComposerOffline).to_owned();
                self.refuse(node, text, cx);
            }
            Some(InputState::Gone) | None => {
                let text = ui::say(cx, Str::ComposerGone).to_owned();
                self.refuse(node, text, cx);
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
                    PickerEvent::Escape => composer.focus_field(node, window, cx),
                    PickerEvent::Close => {}
                }
                cx.notify();
            },
        );
        self.listing = Some((list, sub));
        cx.notify();
    }

    fn focus_field(&self, node: u32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.drafts.get(&node) {
            let handle = draft.field.read(cx).handle();
            window.focus(&handle, cx);
        }
    }

    fn pick(&mut self, current: u32, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.show_node(id, cx);
            store.select(id, cx);
        });
        if id == current {
            self.focus_field(id, window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FOCUS_DELAY).await;
            let _ = this.update_in(cx, |composer, window, cx| {
                composer.focus_field(id, window, cx)
            });
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
        let Some((node, name)) = shown.filter(|(id, _)| self.drafts.contains_key(id)) else {
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
            list.update(cx, |list, _| list.place(z, wide, room, above));
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
        let c = ui::colors(cx);
        let draft = &self.drafts[&node];
        draft.field.update(cx, |field, cx| field.set_scale(z, cx));
        let focused = draft.field.read(cx).focused(window);
        let ready = !draft.field.read(cx).is_blank()
            || !draft.pictures.read(cx).is_empty()
            || draft.filing > 0;
        let sendable = ready && !draft.sending;
        let tool = |id: &'static str, glyph: &'static str, size: f32| {
            div()
                .id(id)
                .flex_none()
                .size(px(size * z))
                .rounded(px(6.0 * z))
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.ink_2)
                .cursor_pointer()
                .hover(move |s| s.bg(c.surface_hover).text_color(c.ink))
                .on_mouse_down(MouseButton::Left, place::stop)
                .child(glyph)
        };
        let target = div()
            .flex()
            .items_center()
            .gap(px(4.0 * z))
            .min_w_0()
            .overflow_hidden()
            .child(">_")
            .child(
                div()
                    .id("composer-name")
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c.ink)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .tooltip(tip(ui::say(cx, Str::ComposerShowNode).to_owned()))
                    .on_mouse_down(MouseButton::Left, place::stop)
                    .on_click(cx.listener(move |composer, _, _, cx| {
                        composer
                            .store
                            .update(cx, |store, cx| store.show_node(node, cx));
                    }))
                    .child(name.to_owned()),
            )
            .child(self.chevron(node, z, cx))
            .child(ui::say(cx, Str::ComposerTo));
        let head = div()
            .flex()
            .items_center()
            .gap(px(6.0 * z))
            .min_w_0()
            .text_size(px(12.0 * z))
            .text_color(c.ink_3)
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |composer, e: &MouseDownEvent, _, cx| composer.grab(e, at, cx)),
            )
            .child(target)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .child(ui::say(cx, Str::ComposerKeys)),
            )
            .children((!home).then(|| {
                tool("composer-reset", "\u{21ba}", 22.0)
                    .tooltip(tip(ui::say(cx, Str::ComposerResetPlace).to_owned()))
                    .on_click(cx.listener(|composer, _, _, cx| composer.reset(cx)))
            }));
        let images = pictures::chips(&draft.pictures, &self.viewer, "composer-chip", node, z, cx);
        let filing = (draft.filing > 0).then(|| {
            div()
                .text_size(px(12.0 * z))
                .text_color(c.ink_3)
                .child(ui::say_with(
                    cx,
                    Str::ComposerFiling,
                    &[("count", &draft.filing.to_string())],
                ))
        });
        let note = draft
            .note
            .clone()
            .map(|text| div().text_size(px(12.0 * z)).text_color(c.err).child(text));
        let line = div()
            .flex()
            .items_end()
            .gap(px(4.0 * z))
            .child(div().flex_1().min_w_0().child(draft.field.clone()))
            .child(
                tool("composer-attach", "+", BUTTON)
                    .text_size(px(18.0 * z))
                    .tooltip(tip(ui::say(cx, Str::ComposerAttachFile).to_owned()))
                    .on_click(cx.listener(move |composer, _, _, cx| {
                        if let Some(draft) = composer.drafts.get(&node) {
                            draft
                                .pictures
                                .update(cx, |pictures, cx| pictures.pick_files(cx));
                        }
                    })),
            )
            .child(
                div()
                    .id("composer-send")
                    .flex_none()
                    .size(px(BUTTON * z))
                    .rounded(px(8.0 * z))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(c.accent)
                    .text_color(c.accent_ink)
                    .when(sendable, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |composer, _, window, cx| {
                                composer.send(node, window, cx)
                            }))
                    })
                    .when(!sendable, |d| d.opacity(DISABLED_ALPHA))
                    .tooltip(tip(ui::say(cx, Str::ComposerSend).to_owned()))
                    .on_mouse_down(MouseButton::Left, place::stop)
                    .child("\u{2191}"),
            );
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
            .border_color(if focused { c.accent } else { c.line_strong })
            .bg(c.panel)
            .text_color(c.ink)
            .text_size(px(13.0 * z))
            .shadow(shadow(c.shadow))
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
            .on_drop(cx.listener(move |composer, paths: &ExternalPaths, _, cx| {
                if let Some(draft) = composer.drafts.get(&node) {
                    draft.pictures.update(cx, |pictures, cx| {
                        pictures.take_paths(paths.paths().to_vec(), cx)
                    });
                }
            }))
            .child(head)
            .children(images)
            .children(filing)
            .children(note)
            .child(line)
    }

    fn chevron(&self, node: u32, z: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ui::colors(cx);
        let open = self.listing.is_some();
        let anchor = self.chevron.clone();
        div()
            .id("composer-chevron")
            .flex_none()
            .size(px(18.0 * z))
            .rounded(px(4.0 * z))
            .flex()
            .items_center()
            .justify_center()
            .text_color(c.ink_2)
            .cursor_pointer()
            .when(open, |d| d.bg(c.surface_hover).text_color(c.ink))
            .hover(move |s| s.bg(c.surface_hover).text_color(c.ink))
            .tooltip(tip(ui::say(cx, Str::ComposerPickNode).to_owned()))
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_click(
                cx.listener(move |composer, _, window, cx| composer.toggle_list(node, window, cx)),
            )
            .child(
                canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| ())
                    .absolute()
                    .size_full(),
            )
            .child(if open { "\u{25b4}" } else { "\u{25be}" })
    }
}
