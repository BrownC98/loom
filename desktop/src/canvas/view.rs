use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, Entity, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, PinchEvent, Pixels, Point, Render, ScrollEvent,
    Styled, Window, canvas, div, point, px, rgb, size,
};

use api::graph::CardView;
use api::graph::pty::Pace;

use super::camera::Camera;
use super::frames::Probe;
use super::grid;
use super::layout::{Rect, Size};
use super::scene::{self, MIN_CARD, Scene};
use crate::link;
use crate::store::Store;
use crate::views::card;
use crate::views::terminal::{self, TerminalView};

const WHEEL_LINE: f32 = 25.0;
const WHEEL_RATE: f32 = 0.002;
const WHEEL_PINCH_BOOST: f32 = 10.0;
const HANDLE: f32 = 18.0;
const TERMINAL_STEP: f32 = 1.03;
const HIDDEN_KEEP: Duration = Duration::from_secs(3);

struct Live {
    view: Entity<TerminalView>,
    hidden: Option<Instant>,
}

struct Resizing {
    node: u32,
    from: Point<Pixels>,
    size: Size,
}

pub struct CanvasView {
    store: Entity<Store>,
    seen: u64,
    scene: Scene,
    resized: HashMap<u32, Size>,
    live: HashMap<u32, Live>,
    focused: Option<u32>,
    resizing: Option<Resizing>,
    fit_pending: bool,
    camera: Camera,
    drag: Option<Point<Pixels>>,
    area: Rc<Cell<Bounds<Pixels>>>,
    probe: Option<Probe>,
    auto: bool,
    started: Instant,
}

fn snapped(zoom: f32) -> f32 {
    TERMINAL_STEP.powf((zoom.ln() / TERMINAL_STEP.ln()).round())
}

fn bounds_of(rect: Rect) -> Bounds<f32> {
    Bounds::new(
        point(rect.at.x as f32, rect.at.y as f32),
        size(rect.size.width as f32, rect.size.height as f32),
    )
}

impl CanvasView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mode = std::env::var("LOOM_DESKTOP_FRAMES").ok();
        cx.observe(&store, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |_, window, _| {
            link::pace(if window.is_window_active() {
                Pace::Front
            } else {
                Pace::Back
            });
        })
        .detach();
        Self {
            store,
            seen: 0,
            scene: Scene::default(),
            resized: HashMap::new(),
            live: HashMap::new(),
            focused: None,
            resizing: None,
            fit_pending: true,
            camera: Camera::HOME,
            drag: None,
            area: Rc::new(Cell::new(Bounds::default())),
            probe: mode.is_some().then(Probe::default),
            auto: mode.as_deref() == Some("auto"),
            started: Instant::now(),
        }
    }

    fn sync(&mut self, cx: &App) {
        let revision = self.store.read(cx).revision();
        if revision != self.seen {
            self.seen = revision;
            self.rebuild(cx);
        }
    }

    fn rebuild(&mut self, cx: &App) {
        if let Some(snapshot) = self.store.read(cx).snapshot() {
            self.scene = scene::build(snapshot, &self.resized);
        }
    }

    fn select(&mut self, node: u32, cx: &mut Context<Self>) {
        self.focused = None;
        self.store.update(cx, |store, cx| store.select(node, cx));
    }

    fn mount(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let (Some(base), selected) = (store.base().map(str::to_owned), store.selected()) else {
            return;
        };
        let area = self.area.get().size;
        if area.width <= px(0.0) {
            return;
        }
        let camera = self.camera;
        let reach = Bounds::new(
            point(-area.width, -area.height),
            size(area.width * 3.0, area.height * 3.0),
        );
        let near: Vec<u32> = self
            .scene
            .cards
            .iter()
            .filter(|c| camera.screen_box(bounds_of(c.rect)).intersects(&reach))
            .map(|c| c.node)
            .collect();
        let now = Instant::now();
        for id in near.iter().copied() {
            let entry = self.live.entry(id).or_insert_with(|| Live {
                view: terminal::open(&base, id, window, cx),
                hidden: None,
            });
            entry.hidden = None;
        }
        let mut waiting = false;
        let mut focused = self.focused;
        self.live.retain(|id, live| {
            if near.contains(id) {
                return true;
            }
            let since = *live.hidden.get_or_insert(now);
            waiting = true;
            let keep = now - since < HIDDEN_KEEP;
            if !keep && focused == Some(*id) {
                focused = None;
            }
            keep
        });
        self.focused = focused;
        if let Some(id) = selected
            && self.focused != Some(id)
            && let Some(live) = self.live.get(&id)
        {
            live.view.update(cx, |view, cx| view.focus(window, cx));
            self.focused = Some(id);
        }
        if waiting {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(HIDDEN_KEEP).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    fn local(&self, window_pos: Point<Pixels>) -> Point<Pixels> {
        window_pos - self.area.get().origin
    }

    fn scripted(&mut self) {
        let t = self.started.elapsed().as_secs_f32();
        self.camera = Camera {
            x: 200.0 * (t * 0.9).sin() - 60.0,
            y: 120.0 * (t * 0.7).cos() - 40.0,
            zoom: 0.45 + 0.4 * (t * 0.5).sin().abs() + 0.25,
        };
    }

    fn fit(&mut self) {
        let area = self.area.get().size;
        let Some(first) = self.scene.cards.first() else {
            return;
        };
        if area.width <= px(0.0) {
            return;
        }
        let mut content = bounds_of(first.rect);
        for rect in self
            .scene
            .repos
            .iter()
            .chain(&self.scene.groups)
            .map(|f| f.rect)
            .chain(self.scene.cards.iter().map(|c| c.rect))
        {
            content = content.union(&bounds_of(rect));
        }
        self.camera = Camera::fit(area, content);
        self.fit_pending = false;
    }

    fn on_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(WHEEL_LINE)).y);
        let boost = if event.modifiers.control {
            WHEEL_PINCH_BOOST
        } else {
            1.0
        };
        let zoom = self.camera.zoom * 2f32.powf(dy * WHEEL_RATE * boost);
        let at = self.local(event.position);
        self.camera.zoom_about(at, zoom);
        cx.notify();
    }

    fn on_pinch(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let at = self.local(event.position);
        self.camera
            .zoom_about(at, self.camera.zoom * (1.0 + event.delta));
        cx.notify();
    }

    fn on_down(&mut self, event: &MouseDownEvent) {
        self.drag = Some(event.position);
    }

    fn on_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            self.resizing = None;
            return;
        }
        if let Some(resizing) = &self.resizing {
            let zoom = f64::from(self.camera.zoom);
            let delta = event.position - resizing.from;
            let next = Size {
                width: (resizing.size.width + f64::from(f32::from(delta.x)) / zoom)
                    .max(MIN_CARD.width),
                height: (resizing.size.height + f64::from(f32::from(delta.y)) / zoom)
                    .max(MIN_CARD.height),
            };
            self.resized.insert(resizing.node, next);
            self.rebuild(cx);
            cx.notify();
            return;
        }
        let Some(last) = self.drag else { return };
        let delta = event.position - last;
        self.camera.pan_by(f32::from(delta.x), f32::from(delta.y));
        self.drag = Some(event.position);
        cx.notify();
    }

    fn on_up(&mut self) {
        self.drag = None;
        self.resizing = None;
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = &self.probe {
            probe.begin();
        }
        if self.fit_pending {
            self.fit();
        }
        if self.auto {
            self.scripted();
            window.request_animation_frame();
        }
        self.mount(window, cx);
        let camera = self.camera;
        let percent = (camera.zoom * 100.0).round() as u32;
        if self.store.read(cx).zoom_percent() != percent {
            let store = self.store.clone();
            cx.defer(move |cx| store.update(cx, |store, cx| store.set_zoom_percent(percent, cx)));
        }
        let area = self.area.clone();
        let frame_end = self.probe.clone();
        for live in self.live.values() {
            live.view
                .update(cx, |view, _| view.set_scale(snapped(camera.zoom)));
        }
        let viewport = Bounds::from_corners(
            point(px(0.0), px(0.0)),
            point(self.area.get().size.width, self.area.get().size.height),
        );
        let store = self.store.read(cx);
        let selected = store.selected();
        let snapshot = store.snapshot();
        let repos = self.scene.repos.iter().map(|f| {
            card::frame(
                &f.label,
                camera.screen_box(bounds_of(f.rect)),
                camera.zoom,
                true,
            )
        });
        let groups = self.scene.groups.iter().map(|f| {
            card::frame(
                &f.label,
                camera.screen_box(bounds_of(f.rect)),
                camera.zoom,
                false,
            )
        });
        let cards = self.scene.cards.iter().filter_map(|placed| {
            let node = snapshot?.nodes.iter().find(|n| n.id.0 == placed.node)?;
            let id = node.id.0;
            let screen = camera.screen_box(bounds_of(placed.rect));
            let body = self
                .live
                .get(&id)
                .filter(|_| screen.intersects(&viewport))
                .map(|live| {
                    let scale = snapped(camera.zoom);
                    div()
                        .w(px((placed.rect.size.width as f32) * scale))
                        .h(px((placed.rect.size.height as f32 - card::HEAD) * scale))
                        .child(live.view.clone())
                        .into_any_element()
                });
            let handle = (node.card_view != Some(CardView::Simple)).then(|| {
                let side = px(HANDLE * camera.zoom);
                div()
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .w(side)
                    .h(side)
                    .bg(rgb(0x4a4d55))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            let size = this
                                .scene
                                .cards
                                .iter()
                                .find(|c| c.node == id)
                                .map(|c| c.rect.size);
                            this.resizing = size.map(|size| Resizing {
                                node: id,
                                from: e.position,
                                size,
                            });
                            cx.stop_propagation();
                        }),
                    )
            });
            Some(
                card::card(node, screen, camera.zoom, selected == Some(id), body)
                    .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.select(id, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .children(handle),
            )
        });
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x14161a))
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| this.on_scroll(e, cx)))
            .on_pinch(cx.listener(|this, e: &PinchEvent, _, cx| this.on_pinch(e, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, _| this.on_down(e)),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.on_up()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.on_up()),
            )
            .child(
                canvas(
                    move |bounds, _, _| area.set(bounds),
                    move |bounds, _, window, _| grid::paint(window, bounds, camera),
                )
                .absolute()
                .size_full(),
            )
            .children(repos)
            .children(groups)
            .children(cards)
            .child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .bottom(px(8.0))
                    .text_size(px(12.0))
                    .text_color(rgb(0x8b9099))
                    .child(format!(
                        "{}  {}  {}%  ({:.0}, {:.0})",
                        store.status(),
                        store.base().map_or("", |_| store.link().label()),
                        (camera.zoom * 100.0).round(),
                        camera.x,
                        camera.y
                    )),
            )
            .children(frame_end.map(|probe| {
                canvas(|_, _, _| (), move |_, _, _, _| probe.end())
                    .absolute()
                    .size(px(1.0))
            }))
    }
}
