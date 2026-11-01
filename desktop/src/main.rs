#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod link;
mod views;
mod vt;
#[cfg(windows)]
mod windows;

use gpui::{App, AppContext, WindowOptions};

fn main() {
    let started = daemon::start();
    gpui_platform::application().run(move |cx: &mut App| {
        cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| canvas::view::CanvasView::new(started, window, cx))
        })
        .expect("could not open the window");
        cx.activate(true);
    });
}
