mod dialog;
mod nav;
mod patch;
mod search;
mod tabs;
mod widgets;

use api::settings::SettingsView;
use gpui::{AnyElement, App, Entity, Global, IntoElement, StyledText, WeakEntity};

pub use dialog::{Dialog, DialogEvent};
pub use patch::{Writer, settings_patch};
pub use tabs::{keep_focus, modal};

use crate::store::Store;
use crate::theme::Theme;

pub struct Ctx<'a> {
    pub view: &'a SettingsView,
    pub defaults: Option<&'a SettingsView>,
    pub writer: Writer,
    pub query: &'a str,
    pub store: &'a Entity<Store>,
    pub dialog: WeakEntity<Dialog>,
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

struct Wanted(nav::Tab);

impl Global for Wanted {}

pub fn want_accounts(cx: &mut App) {
    cx.set_global(Wanted(nav::Tab::Accounts));
}

fn take_wanted(cx: &mut App) -> Option<nav::Tab> {
    cx.has_global::<Wanted>()
        .then(|| cx.remove_global::<Wanted>().0)
}
