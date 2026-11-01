#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod link;
mod views;
mod vt;
#[cfg(windows)]
mod windows;
mod wire;

use gpui::{App, AppContext, WindowOptions};

fn main() {
    let status = match daemon::start() {
        Ok(url) => url,
        Err(error) => error,
    };
    gpui_platform::application().run(move |cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| canvas::camera::CanvasView::new(status))
        })
        .expect("could not open the window");
        cx.activate(true);
    });
}
