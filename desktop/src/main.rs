#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod link;
#[cfg(target_os = "macos")]
mod macos;
mod store;
mod strings;
mod theme;
mod views;
mod vt;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(windows, target_os = "macos")))]
use views::shell::plain as platform;
#[cfg(windows)]
use windows as platform;

use gpui::{App, AppContext};

fn main() {
    let started = daemon::start();
    gpui_platform::application().run(move |cx: &mut App| {
        cx.open_window(platform::window_options(), |window, cx| {
            let store = cx.new(|cx| store::Store::new(started, window, cx));
            let canvas = cx.new(|cx| canvas::view::CanvasView::new(store.clone(), window, cx));
            cx.new(|cx| views::shell::Shell::new(store, canvas.into(), None, None, cx))
        })
        .expect("could not open the window");
        cx.activate(true);
    });
}
