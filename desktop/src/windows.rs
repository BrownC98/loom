use std::os::windows::process::CommandExt;
use std::process::Command;

use gpui::{
    AnyElement, App, Div, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Stateful,
    StatefulInteractiveElement, Styled, TitlebarOptions, Window, WindowControlArea, WindowOptions,
    div, px, rgb,
};

use crate::frame;
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::icons::{Lucide, icon_styled};
use crate::views::tooltip::tip;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CAPTION_WIDTH: f32 = 46.0;
const CAPTION_ICON: f32 = 14.0;
const CAPTION_STROKE: f32 = 1.5;
const CLOSE_HOVER: u32 = 0xc42b1c;
const CLOSE_INK: u32 = 0xffffff;

pub fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(frame::name()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        ..frame::options(cx)
    }
}

pub fn leading_gap() -> Pixels {
    px(0.0)
}

pub fn drag_area(area: Div) -> Div {
    area.window_control_area(WindowControlArea::Drag)
}

fn caption(
    glyph: Lucide,
    label: &str,
    area: WindowControlArea,
    colors: &Colors,
    close: bool,
) -> Stateful<Div> {
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
        .id(glyph.path())
        .occlude()
        .w(px(CAPTION_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .group(glyph.path())
        .window_control_area(area)
        .hover(move |style| style.bg(hover))
        .tooltip(tip(label.to_owned()))
        .child(
            icon_styled(glyph, CAPTION_ICON, colors.ink_3, CAPTION_STROKE, false)
                .group_hover(glyph.path(), move |style| style.text_color(hover_ink)),
        )
}

pub fn captions(window: &Window, colors: &Colors, lang: Lang) -> Option<AnyElement> {
    let (maximize, label) = if window.is_maximized() {
        (Lucide::Copy, Str::TitlebarRestore)
    } else {
        (Lucide::Square, Str::TitlebarMaximize)
    };
    Some(
        div()
            .h_full()
            .flex()
            .child(caption(
                Lucide::Minus,
                Str::TitlebarMinimize.text(lang),
                WindowControlArea::Min,
                colors,
                false,
            ))
            .child(caption(
                maximize,
                label.text(lang),
                WindowControlArea::Max,
                colors,
                false,
            ))
            .child(caption(
                Lucide::X,
                Str::CommonClose.text(lang),
                WindowControlArea::Close,
                colors,
                true,
            ))
            .into_any_element(),
    )
}
