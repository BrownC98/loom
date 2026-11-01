use gpui::{Bounds, Pixels, Rgba, Window, fill, point, px, size};

use super::camera::Camera;

const GAP: f32 = 24.0;
const MIN_SCREEN_GAP: f32 = 14.0;
const DOT: f32 = 1.6;
const DOT_MAX: f32 = 3.0;

pub fn paint(window: &mut Window, bounds: Bounds<Pixels>, camera: Camera, color: Rgba) {
    let mut step = GAP;
    while step * camera.zoom < MIN_SCREEN_GAP {
        step *= 2.0;
    }
    let gap = step * camera.zoom;
    let dot = (DOT * camera.zoom).clamp(1.0, DOT_MAX);
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let start_x = camera.x.rem_euclid(gap);
    let start_y = camera.y.rem_euclid(gap);
    let mut y = start_y;
    while y < height {
        let mut x = start_x;
        while x < width {
            let origin = bounds.origin + point(px(x - dot / 2.0), px(y - dot / 2.0));
            window.paint_quad(fill(Bounds::new(origin, size(px(dot), px(dot))), color));
            x += gap;
        }
        y += gap;
    }
}
