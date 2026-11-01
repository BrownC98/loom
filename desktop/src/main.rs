#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod link;
mod strings;
mod theme;
mod views;
mod vt;
#[cfg(windows)]
mod windows;

use gpui::{App, AppContext, WindowAppearance, WindowOptions};

fn main() {
    let started = daemon::start();
    gpui_platform::application().run(move |cx: &mut App| {
        let settings = api::settings::SettingsView::default();
        let system_dark = matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
        cx.set_global(theme::Theme::resolve(&settings, system_dark));
        cx.set_global(strings::Lang::resolve(settings.language, &sys_locale::get_locale().unwrap_or_default()));
        cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| canvas::view::CanvasView::new(started, window, cx))
        })
        .expect("could not open the window");
        cx.activate(true);
    });
}
