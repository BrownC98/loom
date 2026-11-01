use std::os::windows::process::CommandExt;
use std::process::Command;

use gpui::{
    AnyElement, Div, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Styled,
    TitlebarOptions, Window, WindowControlArea, WindowOptions, div, px, rgb,
};

use crate::theme::Colors;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CAPTION_WIDTH: f32 = 46.0;
const CAPTION_GLYPH: f32 = 10.0;
const CAPTION_FONT: &str = "Segoe MDL2 Assets";
const MINIMIZE: &str = "\u{e921}";
const MAXIMIZE: &str = "\u{e922}";
const RESTORE: &str = "\u{e923}";
const CLOSE: &str = "\u{e8bb}";
const CLOSE_HOVER: u32 = 0xc42b1c;
const CLOSE_INK: u32 = 0xffffff;

pub fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

pub fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("loom".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        ..Default::default()
    }
}

pub fn leading_gap() -> Pixels {
    px(0.0)
}

pub fn drag_area(area: Div) -> Div {
    area.window_control_area(WindowControlArea::Drag)
}

fn caption(glyph: &'static str, area: WindowControlArea, colors: &Colors, close: bool) -> Div {
    let hover = if close {
        Hsla::from(rgb(CLOSE_HOVER))
    } else {
        Hsla::from(colors.surface_hover)
    };
    let hover_ink = if close {
        Hsla::from(rgb(CLOSE_INK))
    } else {
        Hsla::from(colors.ink)
    };
    div()
        .w(px(CAPTION_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .font_family(CAPTION_FONT)
        .text_size(px(CAPTION_GLYPH))
        .text_color(colors.ink_3)
        .window_control_area(area)
        .hover(move |style| style.bg(hover).text_color(hover_ink))
        .child(glyph)
}

pub fn captions(window: &Window, colors: &Colors) -> Option<AnyElement> {
    let maximize = if window.is_maximized() {
        RESTORE
    } else {
        MAXIMIZE
    };
    Some(
        div()
            .h_full()
            .flex()
            .child(caption(MINIMIZE, WindowControlArea::Min, colors, false))
            .child(caption(maximize, WindowControlArea::Max, colors, false))
            .child(caption(CLOSE, WindowControlArea::Close, colors, true))
            .into_any_element(),
    )
}
