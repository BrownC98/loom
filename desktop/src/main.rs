#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod fonts;
mod link;
#[cfg(not(any(windows, target_os = "macos")))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod problem;
mod store;
mod strings;
mod theme;
mod views;
mod vt;
#[cfg(windows)]
mod windows;

#[cfg(not(any(windows, target_os = "macos")))]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

use gpui::{App, AppContext};

fn main() {
    let started = daemon::start();
    gpui_platform::application().run(move |cx: &mut App| {
        fonts::register(cx);
        cx.open_window(platform::window_options(), |window, cx| {
            let store = cx.new(|cx| store::Store::new(started, window, cx));
            let canvas = cx.new(|cx| canvas::view::CanvasView::new(store.clone(), window, cx));
            let sidebar = views::sidebar::sidebar(store.clone(), window, cx);
            let dock = views::dock::dock(store.clone(), window, cx);
            cx.new(|cx| {
                views::shell::Shell::new(
                    store,
                    canvas.into(),
                    Some(sidebar.into()),
                    Some(dock.into()),
                    cx,
                )
            })
        })
        .expect("could not open the window");
        cx.activate(true);
    });
}
