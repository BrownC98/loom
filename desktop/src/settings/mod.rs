mod dialog;
mod nav;
pub mod notify;
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
    beside: Vec<AnyElement>,
    card: bool,
    haystack: String,
    body: Vec<AnyElement>,
    query: String,
}

impl Section {
    pub fn new(head: Option<&str>, ctx: &Ctx) -> Self {
        Self {
            head: head.map(str::to_owned),
            beside: Vec::new(),
            card: false,
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

    pub fn beside(mut self, element: Option<impl IntoElement>) -> Self {
        self.beside
            .extend(element.map(IntoElement::into_any_element));
        self
    }

    pub fn carded(mut self) -> Self {
        self.card = true;
        self
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
