use std::process::Command;

use gpui::{AnyElement, App, Div, Pixels, TitlebarOptions, Window, WindowOptions, px};

use crate::frame;
use crate::strings::Lang;
use crate::theme::Colors;

pub fn hide_console(_: &mut Command) {}

pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(frame::name()),
            ..Default::default()
        }),
        ..frame::options(cx)
    }
}

pub fn leading_gap() -> Pixels {
    px(0.0)
}

pub fn drag_area(area: Div) -> Div {
    area
}

pub fn captions(_: &Window, _: &Colors, _: Lang) -> Option<AnyElement> {
    None
}
