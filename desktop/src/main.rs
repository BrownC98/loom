#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod float;
mod fonts;
mod link;
#[cfg(not(any(windows, target_os = "macos")))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod problem;
mod settings;
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
    gpui_platform::application()
        .with_assets(views::icons::Assets)
        .run(move |cx: &mut App| {
            fonts::register(cx);
            settings::notify::identify(cx);
            cx.open_window(platform::window_options(), |window, cx| {
                let store = cx.new(|cx| store::Store::new(started, window, cx));
                settings::notify::watch(&store, window, cx);
                let inputs = cx.new(|_| float::inputs::Inputs::default());
                let shelf = cx.new(|_| views::pictures::Shelf::default());
                let canvas = cx.new(|cx| {
                    canvas::view::CanvasView::new(store.clone(), inputs.clone(), window, cx)
                });
                let sidebar = views::sidebar::sidebar(store.clone(), window, cx);
                let dock = views::dock::dock(store.clone(), shelf.clone(), window, cx);
                let zoom = views::shell::zoom(store.clone(), window, cx);
                let pill = float::pill::pill(store.clone(), shelf, window, cx);
                let composer = float::composer::composer(store.clone(), inputs, window, cx);
                cx.new(|cx| {
                    views::shell::Shell::new(
                        store,
                        zoom,
                        canvas.into(),
                        Some(sidebar.into()),
                        Some(dock.into()),
                        vec![pill.into(), composer.into()],
                        cx,
                    )
                })
            })
            .expect("could not open the window");
            cx.activate(true);
        });
}
