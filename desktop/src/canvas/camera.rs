use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PinchEvent, Pixels, Point, Render, ScrollEvent, Styled, Window,
    canvas, div, point, px, rgb, size,
};

use super::frames::Probe;
use super::grid;

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 3.5;
const WHEEL_LINE: f32 = 25.0;
const WHEEL_RATE: f32 = 0.002;
const WHEEL_PINCH_BOOST: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Camera {
    pub const HOME: Camera = Camera {
        x: 0.0,
        y: 0.0,
        zoom: 1.0,
    };

    pub fn to_screen(self, graph: Point<f32>) -> Point<Pixels> {
        point(
            px(graph.x * self.zoom + self.x),
            px(graph.y * self.zoom + self.y),
        )
    }

    pub fn to_graph(self, screen: Point<Pixels>) -> Point<f32> {
        point(
            (f32::from(screen.x) - self.x) / self.zoom,
            (f32::from(screen.y) - self.y) / self.zoom,
        )
    }

    pub fn screen_box(self, graph: Bounds<f32>) -> Bounds<Pixels> {
        Bounds {
            origin: self.to_screen(graph.origin),
            size: size(
                px(graph.size.width * self.zoom),
                px(graph.size.height * self.zoom),
            ),
        }
    }

    pub fn zoom_about(&mut self, at: Point<Pixels>, zoom: f32) {
        let next = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let anchor = self.to_graph(at);
        self.zoom = next;
        self.x = f32::from(at.x) - anchor.x * next;
        self.y = f32::from(at.y) - anchor.y * next;
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }
}

const CARD: (f32, f32) = (480.0, 320.0);
const GAP: f32 = 80.0;

fn test_cards() -> Vec<(usize, Bounds<f32>)> {
    (0..9)
        .map(|i| {
            let (col, row) = ((i % 3) as f32, (i / 3) as f32);
            let origin = point(40.0 + col * (CARD.0 + GAP), 40.0 + row * (CARD.1 + GAP));
            (i, Bounds::new(origin, size(CARD.0, CARD.1)))
        })
        .collect()
}

pub struct CanvasView {
    status: String,
    camera: Camera,
    drag: Option<Point<Pixels>>,
    area: Rc<Cell<Bounds<Pixels>>>,
    probe: Option<Probe>,
    auto: bool,
    started: Instant,
}

impl CanvasView {
    pub fn new(status: String) -> Self {
        let mode = std::env::var("LOOM_DESKTOP_FRAMES").ok();
        Self {
            status,
            camera: Camera::HOME,
            drag: None,
            area: Rc::new(Cell::new(Bounds::default())),
            probe: mode.is_some().then(Probe::default),
            auto: mode.as_deref() == Some("auto"),
            started: Instant::now(),
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
        let Some(last) = self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let delta = event.position - last;
        self.camera.pan_by(f32::from(delta.x), f32::from(delta.y));
        self.drag = Some(event.position);
        cx.notify();
    }

    fn on_up(&mut self, _: &MouseUpEvent) {
        self.drag = None;
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = &self.probe {
            probe.begin();
        }
        if self.auto {
            self.scripted();
            window.request_animation_frame();
        }
        let camera = self.camera;
        let area = self.area.clone();
        let frame_end = self.probe.clone();
        let cards = test_cards().into_iter().map(|(i, graph)| {
            let boxed = camera.screen_box(graph);
            div()
                .absolute()
                .left(boxed.origin.x)
                .top(boxed.origin.y)
                .w(boxed.size.width)
                .h(boxed.size.height)
                .rounded(px(8.0 * camera.zoom))
                .border(px(camera.zoom.max(0.5)))
                .border_color(rgb(0x4a4d55))
                .bg(rgb(0x1d2025))
                .text_color(rgb(0xe6e8eb))
                .text_size(px(14.0 * camera.zoom))
                .p(px(12.0 * camera.zoom))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(format!("card {i}"))
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
                cx.listener(|this, e: &MouseUpEvent, _, _| this.on_up(e)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, _| this.on_up(e)),
            )
            .child(
                canvas(
                    move |bounds, _, _| area.set(bounds),
                    move |bounds, _, window, _| grid::paint(window, bounds, camera),
                )
                .absolute()
                .size_full(),
            )
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
