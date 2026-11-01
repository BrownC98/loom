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
    let started = daemon::start();
    gpui_platform::application().run(move |cx: &mut App| {
        if std::env::var_os(views::terminal::NODE_ENV).is_some() {
            cx.open_window(WindowOptions::default(), |window, cx| {
                views::terminal::show(started, window, cx)
            })
            .expect("could not open the window");
        } else {
            let status = started.unwrap_or_else(|error| error);
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| canvas::camera::CanvasView::new(status))
            })
            .expect("could not open the window");
        }
        cx.activate(true);
    });
}
