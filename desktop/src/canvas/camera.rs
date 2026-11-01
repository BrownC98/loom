use gpui::{Bounds, Pixels, Point, Size, point, px, size};

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 3.5;
const FIT_MARGIN: f32 = 40.0;

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
        let zoom = ((width - 2.0 * FIT_MARGIN) / content.size.width)
            .min((height - 2.0 * FIT_MARGIN) / content.size.height)
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
