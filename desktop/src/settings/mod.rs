#![allow(dead_code)]

mod dialog;
mod nav;
mod patch;
mod search;
mod tabs;
mod widgets;

use api::settings::SettingsView;
use gpui::{AnyElement, App, IntoElement, StyledText};

pub use dialog::{Dialog, DialogEvent};
pub use patch::Writer;

use crate::theme::Theme;

pub struct Ctx<'a> {
    pub view: &'a SettingsView,
    pub defaults: Option<&'a SettingsView>,
    pub writer: Writer,
    pub query: &'a str,
}

pub struct Section {
    head: Option<String>,
    haystack: String,
    body: Vec<AnyElement>,
    query: String,
}

impl Section {
    pub fn new(head: Option<&str>, ctx: &Ctx) -> Self {
        Self {
            head: head.map(str::to_owned),
            haystack: head.unwrap_or_default().to_owned(),
            body: Vec::new(),
            query: ctx.query.to_owned(),
        }
    }

    pub fn text(&mut self, text: &str, cx: &App) -> StyledText {
        self.haystack.push('\n');
        self.haystack.push_str(text);
        search::mark(text, &self.query, &cx.global::<Theme>().colors)
    }

    pub fn child(mut self, element: impl IntoElement) -> Self {
        self.body.push(element.into_any_element());
        self
    }
}
