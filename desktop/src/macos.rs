use std::process::Command;

use gpui::{AnyElement, App, Div, Pixels, TitlebarOptions, Window, WindowOptions, point, px};

use crate::frame;
use crate::strings::Lang;
use crate::theme::Colors;

const TRAFFIC_LIGHT: f32 = 13.0;
const TRAFFIC_LIGHT_GAP: f32 = 64.0;

pub fn hide_console(_: &mut Command) {}

pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(frame::name()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(TRAFFIC_LIGHT), px(TRAFFIC_LIGHT))),
        }),
        app_owns_titlebar_drag: true,
        ..frame::options(cx)
    }
}

pub fn leading_gap() -> Pixels {
    px(TRAFFIC_LIGHT_GAP)
}

pub fn drag_area(area: Div) -> Div {
    area
}

pub fn captions(_: &Window, _: &Colors, _: Lang) -> Option<AnyElement> {
    None
}
