use std::time::{Duration, Instant};

use gpui::{Bounds, Pixels, Point, Size, point, px, size};

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 3.5;
const FIT_PADDING: f32 = 0.1;
const GLIDE: Duration = Duration::from_millis(300);

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

    pub fn fit(area: Size<Pixels>, content: Bounds<f32>) -> Camera {
        let (width, height) = (f32::from(area.width), f32::from(area.height));
        let margin = |view: f32| 2.0 * ((view - view / (1.0 + FIT_PADDING)) * 0.5).floor();
        let zoom = ((width - margin(width)) / content.size.width)
            .min((height - margin(height)) / content.size.height)
            .clamp(MIN_ZOOM, 1.0);
        Camera {
            x: (width - content.size.width * zoom) / 2.0 - content.origin.x * zoom,
            y: (height - content.size.height * zoom) / 2.0 - content.origin.y * zoom,
            zoom,
        }
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Glide {
    from: Camera,
    to: Camera,
    start: Instant,
}

impl Glide {
    pub fn new(from: Camera, to: Camera, now: Instant) -> Self {
        Self {
            from,
            to,
            start: now,
        }
    }

    pub fn at(&self, now: Instant) -> (Camera, bool) {
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / GLIDE.as_secs_f32())
            .min(1.0);
        if t >= 1.0 {
            return (self.to, true);
        }
        let k = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        };
        let mix = |a: f32, b: f32| a + (b - a) * k;
        let camera = Camera {
            x: mix(self.from.x, self.to.x),
            y: mix(self.from.y, self.to.y),
            zoom: mix(self.from.zoom, self.to.zoom),
        };
        (camera, false)
    }

    pub fn shift(&mut self, dx: f32, dy: f32) {
        self.from.pan_by(dx, dy);
        self.to.pan_by(dx, dy);
    }
}
