use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    Bounds, Context, Entity, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, PinchEvent, Pixels, Point, Render, ScrollEvent,
    Styled, Window, canvas, div, point, px, rgb, size,
};

use super::camera::Camera;
use super::frames::Probe;
use super::grid;
use super::layout::{Rect, Size};
use super::scene::{self, MIN_CARD, Scene};
use crate::link::{self, Graph};
use crate::views::card;
use crate::views::terminal::{self, TerminalView};
use crate::wire::Snapshot;

pub const NODE_ENV: &str = "LOOM_DESKTOP_NODE";
const WHEEL_LINE: f32 = 25.0;
const WHEEL_RATE: f32 = 0.002;
const WHEEL_PINCH_BOOST: f32 = 10.0;
const HANDLE: f32 = 18.0;
const TERMINAL_STEP: f32 = 1.03;

struct Resizing {
    node: u32,
    from: Point<Pixels>,
    size: Size,
}

pub struct CanvasView {
    base: Option<String>,
    status: String,
    snapshot: Option<Snapshot>,
    scene: Scene,
    resized: HashMap<u32, Size>,
    live: Option<(u32, Entity<TerminalView>)>,
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
    pub fn new(started: Result<String, String>, window: &Window, cx: &mut Context<Self>) -> Self {
        let mode = std::env::var("LOOM_DESKTOP_FRAMES").ok();
        let (base, status) = match started {
            Ok(base) => {
                let (graph, inbox) = async_channel::unbounded();
                let status = link::watch(&base, graph)
                    .err()
                    .unwrap_or_else(|| base.clone());
                cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = inbox.recv().await {
                        if this
                            .update_in(cx, |view, window, cx| view.apply(event, window, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
                (Some(base), status)
            }
            Err(error) => (None, error),
        };
        Self {
            base,
            status,
            snapshot: None,
            scene: Scene::default(),
            resized: HashMap::new(),
            live: None,
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

    fn apply(&mut self, event: Graph, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Graph::Loaded(snapshot) => {
                self.snapshot = Some(*snapshot);
                self.rebuild();
                let present = |id: u32| self.scene.cards.iter().any(|c| c.node == id);
                if self.live.as_ref().is_none_or(|(id, _)| !present(*id)) {
                    self.live = None;
                    let wanted = std::env::var(NODE_ENV)
                        .ok()
                        .and_then(|id| id.parse().ok())
                        .filter(|&id| present(id));
                    if let Some(id) = wanted.or(self.scene.cards.first().map(|c| c.node)) {
                        self.select(id, window, cx);
                    }
                }
            }
            Graph::Failed(error) => {
                self.status = if error.is_empty() {
                    "connection closed".to_owned()
                } else {
                    error
                }
            }
        }
        cx.notify();
    }

    fn rebuild(&mut self) {
        if let Some(snapshot) = &self.snapshot {
            self.scene = scene::build(snapshot, &self.resized);
        }
    }

    fn select(&mut self, node: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.as_ref().is_some_and(|(id, _)| *id == node) {
            return;
        }
        if let Some(base) = &self.base {
            let view = terminal::open(base, node, window, cx);
            self.live = Some((node, view));
        }
        cx.notify();
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
            self.rebuild();
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
        let camera = self.camera;
        let area = self.area.clone();
        let frame_end = self.probe.clone();
        if let Some((_, view)) = &self.live {
            view.update(cx, |view, _| view.set_scale(snapped(camera.zoom)));
        }
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
            let snapshot = self.snapshot.as_ref()?;
            let node = snapshot.nodes.iter().find(|n| n.id == placed.node)?;
            let id = node.id;
            let live = self.live.as_ref().filter(|(live, _)| *live == id);
            let body = live.map(|(_, view)| {
                let scale = snapped(camera.zoom);
                div()
                    .w(px((placed.rect.size.width as f32) * scale))
                    .h(px((placed.rect.size.height as f32 - card::HEAD) * scale))
                    .child(view.clone())
                    .into_any_element()
            });
            let handle = (!node.simple).then(|| {
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
                card::card(
                    node,
                    camera.screen_box(bounds_of(placed.rect)),
                    camera.zoom,
                    live.is_some(),
                    body,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.select(id, window, cx);
                        cx.stop_propagation();
                    }),
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
                        "{}  {}%  ({:.0}, {:.0})",
                        self.status,
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
