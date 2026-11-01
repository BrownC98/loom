use gpui::{App, Bounds, SharedString, WindowBounds, WindowOptions, px, size};

use crate::link::VERSION;

const SIZE: (f32, f32) = (1280.0, 800.0);
const MIN: (f32, f32) = (900.0, 600.0);

pub fn name() -> SharedString {
    format!("loom {VERSION}").into()
}

pub fn options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(SIZE.0), px(SIZE.1)),
            cx,
        ))),
        window_min_size: Some(size(px(MIN.0), px(MIN.1))),
        ..Default::default()
    }
}
