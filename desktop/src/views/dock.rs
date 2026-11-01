use api::graph::docs::{DocGroup, DocRow};
use api::graph::waits::ClosedWait;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use self::answers::Answers;
use super::field::{self, Field};
use super::rail;
use super::sidebar::icons::{Glyph, icon};
use crate::store::Store;
use crate::strings::Str;
use crate::views::pictures::Shelf;

pub(crate) mod answers;
mod arrange;
mod body;
mod commits;
mod docs;
pub mod events;
mod history;
mod lanes;
pub(crate) mod questions;
mod strip;
mod strokes;
pub mod ui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Questions,
    Docs,
    Events,
    Commits,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Questions, Tab::Docs, Tab::Events, Tab::Commits];

    fn id(self) -> &'static str {
        match self {
            Tab::Questions => "tab-questions",
            Tab::Docs => "tab-docs",
            Tab::Events => "tab-events",
            Tab::Commits => "tab-commits",
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Tab::Questions => Glyph::Bell,
            Tab::Docs => Glyph::Document,
            Tab::Events => Glyph::Lines,
            Tab::Commits => Glyph::Commit,
        }
    }

    fn label(self) -> Str {
        match self {
            Tab::Questions => Str::InboxTab,
            Tab::Docs => Str::DocsTitle,
            Tab::Events => Str::EventsTitle,
            Tab::Commits => Str::AppCommitGraph,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Inbox {
    New,
    History,
}

#[derive(Default)]
struct History {
    rows: Vec<ClosedWait>,
    more: bool,
    loaded: bool,
    error: Option<String>,
}

struct Docs {
    order: u64,
    loaded: u64,
    rows: Option<Vec<DocRow>>,
    error: Option<String>,
    hidden: Vec<DocGroup>,
    query: Entity<Field>,
}

pub struct Dock {
    store: Entity<Store>,
    tab: Tab,
    inbox: Inbox,
    answers: Entity<Answers>,
    open_key: String,
    arrange_menu: bool,
    history: History,
    docs: Docs,
    commits: commits::Commits,
    events: events::Events,
    fault: Option<String>,
    subscriptions: Vec<Subscription>,
}

pub fn dock(
    store: Entity<Store>,
    shelf: Entity<Shelf>,
    _: &mut Window,
    cx: &mut gpui::App,
) -> Entity<Dock> {
    cx.new(|cx| {
        let placeholder = ui::say(cx, Str::DocsFilterPlaceholder);
        let query = field::boxed(placeholder, false, false, cx);
        let mut dock = Dock {
            store: store.clone(),
            tab: Tab::Questions,
            inbox: Inbox::New,
            answers: cx.new(|cx| Answers::new(store.clone(), shelf, cx)),
            open_key: String::new(),
            arrange_menu: false,
            history: History::default(),
            docs: Docs {
                order: 0,
                loaded: 0,
                rows: None,
                error: None,
                hidden: Vec::new(),
                query: query.clone(),
            },
            commits: commits::Commits::default(),
            events: events::Events::new(cx),
            fault: None,
            subscriptions: Vec::new(),
        };
        dock.subscriptions
            .push(cx.observe(&store, |dock: &mut Dock, _, cx| dock.sync(cx)));
        dock.subscriptions
            .push(cx.observe(&store, |dock: &mut Dock, _, cx| dock.commits_follow(cx)));
        dock.subscriptions
            .push(cx.observe(&dock.answers, |_, _, cx| cx.notify()));
        dock.subscriptions
            .push(cx.observe(&query, |_, _, cx| cx.notify()));
        dock.subscriptions
            .push(cx.observe(dock.events.query(), |dock, _, cx| dock.query_changed(cx)));
        dock.subscriptions
            .push(cx.subscribe(&store, |dock, _, event, cx| dock.feed(event, cx)));
        dock.sync(cx);
        dock
    })
}

impl Dock {
    fn choose_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.fault = None;
        if tab != Tab::Events {
            self.store
                .update(cx, |store, cx| store.set_picked(None, cx));
        }
        if tab == Tab::Docs {
            self.load_docs(cx);
        }
        if tab == Tab::Events {
            self.load_events(cx);
        }
        if tab == Tab::Commits {
            self.commits_enter(cx);
        }
        cx.notify();
    }

    fn waiting(&self, cx: &Context<Self>) -> usize {
        self.store
            .read(cx)
            .snapshot()
            .map_or(0, |s| s.nodes.iter().map(|n| n.waits.len()).sum())
    }
}

impl Render for Dock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.store.read(cx).settings().closed_panes.dock {
            return self.strip(cx).into_any_element();
        }
        let c = ui::colors(cx);
        let waiting = self.waiting(cx);
        let current = self.tab;
        let tabs = Tab::ALL.map(|tab| {
            let on = current == tab;
            let count = if tab == Tab::Questions { waiting } else { 0 };
            let color = if on { c.ink } else { c.ink_2 };
            let label = ui::say(cx, tab.label());
            div()
                .id(tab.id())
                .flex()
                .items_center()
                .gap(px(ui::GAP))
                .h(px(36.0))
                .px(px(ui::PAD))
                .min_w_0()
                .border_b_2()
                .border_color(if on {
                    gpui::Hsla::from(c.accent)
                } else {
                    gpui::transparent_black()
                })
                .text_size(px(ui::TEXT))
                .text_color(color)
                .when(on, |tab| tab.flex_none().font_weight(FontWeight::SEMIBOLD))
                .cursor_pointer()
                .child(icon(tab.glyph(), 14.0, color))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .children((count > 0).then(|| {
                    div()
                        .flex_none()
                        .min_w(px(18.0))
                        .h(px(18.0))
                        .px(px(5.0))
                        .rounded(px(9.0))
                        .bg(c.notice)
                        .text_color(c.notice_ink)
                        .text_size(px(11.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(count.to_string())
                }))
                .on_click(cx.listener(move |dock, _, _, cx| dock.choose_tab(tab, cx)))
        });
        let panel = match self.tab {
            Tab::Questions => self.questions(cx).into_any_element(),
            Tab::Docs => self.documents(cx).into_any_element(),
            Tab::Events => self.events(cx).into_any_element(),
            Tab::Commits => self.commits_panel(cx).into_any_element(),
        };
        let store = self.store.clone();
        let collapse = rail::button(
            "dock-collapse",
            Glyph::Panel {
                right: true,
                open: false,
            },
            rail::titled(ui::say(cx, Str::DockCollapse), "]"),
            rail::SMALL_BUTTON,
            &c,
        )
        .on_click(move |_, _, cx| {
            store.update(cx, |store, cx| store.set_pane_open("dock", false, cx))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.panel)
            .text_color(c.ink)
            .text_size(px(ui::TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(12.0))
                    .pr(px(ui::PAD))
                    .pt(px(ui::PAD))
                    .child(
                        div()
                            .text_size(px(ui::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.ink_3)
                            .child(ui::say(cx, Str::DockView)),
                    )
                    .child(collapse),
            )
            .child(self.arrange_bar(cx))
            .child(
                div()
                    .flex()
                    .gap(px(2.0))
                    .px(px(ui::PAD))
                    .border_b_1()
                    .border_color(c.line)
                    .children(tabs),
            )
            .children(self.fault.clone().map(|fault| ui::note(fault, true, cx)))
            .child(
                div()
                    .id("dock-panel")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(panel),
            )
            .into_any_element()
    }
}
