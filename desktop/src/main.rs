#![cfg_attr(windows, windows_subsystem = "windows")]

mod canvas;
mod daemon;
mod link;
mod views;
mod vt;
#[cfg(windows)]
mod windows;
mod wire;

use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, WindowOptions,
    div, rgb,
};

struct Root {
    status: String,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x14161a))
            .text_color(rgb(0xe6e8eb))
            .child(self.status.clone())
    }
}

fn main() {
    let status = match daemon::start() {
        Ok(url) => url,
        Err(error) => error,
    };
    gpui_platform::application().run(move |cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| Root { status })
        })
        .expect("could not open the window");
        cx.activate(true);
    });
}
