use gpui::{AnyElement, Div, Pixels, TitlebarOptions, Window, WindowOptions, px};

use crate::theme::Colors;

pub fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("loom".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn leading_gap() -> Pixels {
    px(0.0)
}

pub fn drag_area(area: Div) -> Div {
    area
}

pub fn captions(_: &Window, _: &Colors) -> Option<AnyElement> {
    None
}
