use std::cell::Cell;
use std::rc::Rc;

use api::graph::docs::{DocGroup, DocRow};
use api::graph::waits::ClosedWait;
use api::graph::{CardView, NodeView};
use api::settings::DefaultCardView;
use gpui::{
    AppContext, Bounds, Context, Entity, FocusHandle, IntoElement, ParentElement, Pixels, Render,
    Styled, Subscription, UniformListScrollHandle, Window, div, px,
};

use self::answers::Answers;
use super::anchor::Anchors;
use super::field::{self, Field, FieldEvent};
use super::icons::Lucide;
use super::scrollbar::Bar;
use super::terminal::TerminalView;
use crate::canvas::repos::chosen_repo;
use crate::store::Store;
use crate::strings::Str;
use crate::views::inputs::Inputs;
use crate::views::pictures::Shelf;

pub(crate) mod answers;
mod arrange;
mod body;
mod commits;
mod docs;
pub(crate) mod entry;
pub mod events;
pub(crate) mod find;
mod history;
mod lanes;
pub(crate) mod questions;
mod strip;
mod strokes;
mod tabs;
mod terminal;
mod tools;
pub mod ui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Questions,
    Docs,
    Events,
    Commits,
    Terminal,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Questions,
        Tab::Docs,
        Tab::Events,
        Tab::Commits,
        Tab::Terminal,
    ];

    fn id(self) -> &'static str {
        match self {
            Tab::Questions => "tab-questions",
            Tab::Docs => "tab-docs",
            Tab::Events => "tab-events",
            Tab::Commits => "tab-commits",
            Tab::Terminal => "tab-terminal",
        }
    }

    fn glyph(self) -> Lucide {
        match self {
            Tab::Questions => Lucide::Bell,
            Tab::Docs => Lucide::FileText,
            Tab::Events => Lucide::AlignLeft,
            Tab::Commits => Lucide::GitCommitVertical,
            Tab::Terminal => Lucide::SquareTerminal,
        }
    }

    fn label(self) -> Str {
        match self {
            Tab::Questions => Str::InboxTab,
            Tab::Docs => Str::DocsTitle,
            Tab::Events => Str::EventsTitle,
            Tab::Commits => Str::AppCommitGraph,
            Tab::Terminal => Str::DockTerminal,
        }
    }
}

pub(crate) fn card_view_of(node: &NodeView, default: DefaultCardView) -> CardView {
    node.card_view.unwrap_or(match default {
        DefaultCardView::Simple => CardView::Simple,
        DefaultCardView::Standard => CardView::Standard,
        DefaultCardView::RootTerminal if node.parent.is_none() => CardView::Standard,
        DefaultCardView::RootTerminal => CardView::Simple,
    })
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
    query: String,
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
    shown: Tab,
    arrange_menu: Option<bool>,
    menu_focus: Vec<FocusHandle>,
    caret_focus: FocusHandle,
    caret: Rc<Cell<Bounds<Pixels>>>,
    tab_focus: Vec<FocusHandle>,
    terminal: Option<(u32, Entity<TerminalView>)>,
    anchors: Anchors,
    seen_selected: Option<u32>,
    edits: u64,
    renaming: Option<terminal::Rename>,
    inputs: Entity<Inputs>,
    shared: Entity<Inputs>,
    history: History,
    search: Entity<Field>,
    find: find::State,
    docs: Docs,
    commits: commits::Commits,
    commit_bar: Bar<UniformListScrollHandle>,
    panel_bar: Bar,
    events: events::Events,
    fault: Option<String>,
    subscriptions: Vec<Subscription>,
}

pub fn dock(
    store: Entity<Store>,
    shelf: Entity<Shelf>,
    shared: Entity<Inputs>,
    _: &mut Window,
    cx: &mut gpui::App,
) -> Entity<Dock> {
    cx.new(|cx| {
        let query = field::new(Str::DocsFilterPlaceholder, cx);
        let search = field::new(Str::InboxSearchPlaceholder, cx);
        let mut dock = Dock {
            store: store.clone(),
            tab: Tab::Questions,
            inbox: Inbox::New,
            answers: cx.new(|cx| Answers::new(store.clone(), shelf, cx)),
            open_key: String::new(),
            shown: Tab::Questions,
            arrange_menu: None,
            menu_focus: (0..5).map(|_| cx.focus_handle()).collect(),
            caret_focus: cx.focus_handle(),
            caret: arrange::new_caret(),
            tab_focus: (0..Tab::ALL.len()).map(|_| cx.focus_handle()).collect(),
            terminal: None,
            anchors: Anchors::default(),
            seen_selected: None,
            edits: 0,
            renaming: None,
            inputs: cx.new(|_| Inputs::default()),
            shared,
            history: History::default(),
            search: search.clone(),
            find: find::State::default(),
            docs: Docs {
                order: 0,
                loaded: 0,
                rows: None,
                error: None,
                hidden: Vec::new(),
                query: query.clone(),
            },
            commits: commits::Commits::default(),
            commit_bar: Bar::over(UniformListScrollHandle::new()),
            panel_bar: Bar::new(),
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
            .push(cx.observe(&search, |dock, _, cx| dock.search_changed(cx)));
        dock.subscriptions
            .push(cx.subscribe(&search, |dock, field, event, cx| match event {
                FieldEvent::Escape => field.update(cx, |field, cx| field.clear(cx)),
                FieldEvent::Enter => dock.find_step(1, cx),
                FieldEvent::Step(delta) => dock.find_step(delta.signum(), cx),
                FieldEvent::Changed => {}
            }));
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
        self.arrange_menu = None;
        self.inbox = Inbox::New;
        self.search.update(cx, |field, cx| field.clear(cx));
        self.panel_bar.reset();
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

    fn docked(&self, cx: &Context<Self>) -> Option<NodeView> {
        let store = self.store.read(cx);
        let id = store.selected()?;
        let default = store.settings().card_view;
        store
            .snapshot()?
            .nodes
            .iter()
            .find(|node| node.id.0 == id && card_view_of(node, default) == CardView::Simple)
            .cloned()
    }

    fn waiting(&self, cx: &Context<Self>) -> usize {
        self.store
            .read(cx)
            .snapshot()
            .map_or(0, |s| s.nodes.iter().map(|n| n.waits.len()).sum())
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.store.read(cx).settings().closed_panes.dock {
            return self.strip(cx).into_any_element();
        }
        let c = ui::colors(cx);
        let waiting = self.waiting(cx);
        let docked = self.docked(cx);
        let tabs: Vec<Tab> = if docked.is_some() {
            Tab::ALL.to_vec()
        } else {
            Tab::ALL[..4].to_vec()
        };
        let shown = if tabs.contains(&self.tab) {
            self.tab
        } else {
            Tab::Questions
        };
        self.shown = shown;
        if shown != Tab::Terminal {
            self.terminal = None;
            self.renaming = None;
        }
        let repo =
            self.store.read(cx).snapshot().is_some_and(|s| {
                chosen_repo(&s.repos, self.store.read(cx).settings().repo).is_some()
            });
        let tab_bar = self.tab_bar(&tabs, shown, waiting, cx);
        let panel = match (shown, docked) {
            (Tab::Terminal, Some(node)) => {
                self.terminal_panel(&node, window, cx).into_any_element()
            }
            (Tab::Docs, _) => self.documents(cx).into_any_element(),
            (Tab::Events, _) => self.events(cx).into_any_element(),
            (Tab::Commits, _) => self.commits_panel(cx).into_any_element(),
            _ => self.questions(cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.panel)
            .text_color(c.ink)
            .text_size(px(ui::TEXT))
            .line_height(gpui::relative(1.5))
            .child(self.tools(repo, cx))
            .child(tab_bar)
            .children(self.fault.clone().map(|fault| ui::note(fault, true, cx)))
            .child(div().flex_1().min_h_0().child(panel))
            .into_any_element()
    }
}
