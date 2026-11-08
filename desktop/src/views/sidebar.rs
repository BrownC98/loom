mod acts;
mod add_repo;
mod calls;
mod pop;
mod rename;
mod tree;

use std::collections::HashSet;

use gpui::{
    AnyElement, AnyView, App, AppContext, Bounds, ClickEvent, Context, Entity, FocusHandle,
    FontWeight, HighlightStyle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Pixels, Point, Render, Rgba, SharedString, StatefulInteractiveElement, Styled,
    StyledText, Subscription, Window, div, linear_color_stop, linear_gradient, point, px, relative,
};

use acts::{Act, Button};
use add_repo::AddRepo;

use super::field::{self, Field, FieldEvent};
use super::rail;
use super::scrollbar::Bar;
use super::shell::OpenSettings;
use crate::views::icons::{Lucide, agent_mark, icon, icon_styled};
pub use tree::Dot;
use tree::{Body, Built, Found, Key, Row, Tag, Tone};

use api::graph::Place;
use api::ids::RepoId;
use api::profile::AgentView;
use api::repos::PlaceView;

use crate::canvas::popover::NewNodeView;
use crate::fonts;
use crate::link;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::anchor::Anchors;
use crate::views::cardlink;
use crate::views::dotted;
use crate::views::tooltip::tip;
use crate::views::worktree::{Closed, DeleteWorktreeView, Doomed, NewWorktreeView};

const STEP: f32 = 12.0;
const MAX_DEPTH: usize = 8;
const ROW: f32 = 30.0;
const NODE_ROW: f32 = 44.0;
const ICON: f32 = 14.0;
const POP_GAP: f32 = 8.0;
const ADD_TOP: f32 = 84.0;
const ADD_LEFT: f32 = 12.0;
const FADE: f32 = 16.0;
const AREA: &str = "side";

struct Pick {
    selected: Option<u32>,
    ring: Option<Key>,
}

struct Open {
    view: AnyView,
    host: Option<Point<Pixels>>,
    _closed: Subscription,
}

pub struct Sidebar {
    store: Entity<Store>,
    field: Entity<Field>,
    focus: FocusHandle,
    marks_scroll: Bar,
    tree_scroll: Bar,
    closed: HashSet<Key>,
    cursor: Option<Key>,
    keyboard: bool,
    reveal: bool,
    seen: Option<u32>,
    at: usize,
    refusal: Option<(Key, String)>,
    want_find: bool,
    agents: Vec<AgentView>,
    asked: bool,
    pop: Option<Open>,
    renaming: Option<rename::Renaming>,
    renames: u64,
    refocus: bool,
    anchors: Anchors,
}

pub fn sidebar(store: Entity<Store>, _: &mut Window, cx: &mut App) -> Entity<Sidebar> {
    cx.new(|cx| Sidebar::new(store, cx))
}

impl Sidebar {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |this, store, cx| {
            let selected = store.read(cx).selected();
            if selected != this.seen {
                this.seen = selected;
                this.cursor = selected.map(Key::Node);
                this.reveal = true;
            }
            cx.notify();
        })
        .detach();
        let field = field::new(Str::FindOpen, cx);
        cx.subscribe(&field, |this, _, event: &FieldEvent, cx| {
            this.find(event, cx);
        })
        .detach();
        Self {
            store,
            field,
            focus: cx.focus_handle().tab_stop(true),
            marks_scroll: Bar::new(),
            tree_scroll: Bar::new(),
            closed: HashSet::new(),
            cursor: None,
            keyboard: false,
            reveal: false,
            seen: None,
            at: 0,
            refusal: None,
            want_find: false,
            agents: Vec::new(),
            asked: false,
            pop: None,
            renaming: None,
            renames: 0,
            refocus: false,
            anchors: Anchors::default(),
        }
    }

    fn load_agents(&mut self, cx: &mut Context<Self>) {
        if self.asked || self.store.read(cx).base().is_none() {
            return;
        }
        self.asked = true;
        let task = self.store.update(cx, |store, cx| store.agents(cx));
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(agents) = loaded {
                    this.agents = agents;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn built(&self, cx: &App) -> Option<Built> {
        let snapshot = self.store.read(cx).snapshot()?;
        Some(tree::build(
            snapshot,
            &self.agents,
            *cx.global::<Lang>(),
            &self.closed,
            self.field.read(cx).text(),
            self.at,
        ))
    }

    fn marks_open(&self, cx: &App) -> bool {
        !self.store.read(cx).settings().closed_panes.bookmarks
    }

    fn find(&mut self, event: &FieldEvent, cx: &mut Context<Self>) {
        match event {
            FieldEvent::Changed => {
                self.at = 0;
                self.cursor = self.built(cx).and_then(|b| b.hits.first().cloned());
                self.reveal = true;
            }
            FieldEvent::Step(_) | FieldEvent::Enter => {
                let step = match event {
                    FieldEvent::Step(step) => *step,
                    _ => 1,
                };
                let Some(built) = self.built(cx) else { return };
                if !built.hits.is_empty() {
                    let len = built.hits.len() as isize;
                    self.at = (self.at as isize + step).rem_euclid(len) as usize;
                    self.cursor = built.hits.get(self.at).cloned();
                    self.reveal = true;
                }
            }
            FieldEvent::Escape => {
                self.field.update(cx, |field, cx| field.clear(cx));
                self.reveal = true;
            }
        }
        let key_of = |key: &Key| match key {
            Key::Node(id) | Key::Mark(id) => id.to_string(),
            Key::Repo(id) => format!("repo:{id}"),
            Key::Group(_, path) => path.clone(),
        };
        let built = self.built(cx).filter(|b| !b.hits.is_empty());
        let hit = built.as_ref().and_then(|b| b.hits.get(self.at)).cloned();
        let current = hit.as_ref().map(key_of);
        let moved = current.is_some() && self.store.read(cx).current() != current.as_deref();
        let found = built.map(|b| b.hits.iter().map(key_of).collect());
        self.store
            .update(cx, |store, cx| store.set_found(found, current, cx));
        if let Some(hit) = hit.filter(|_| moved) {
            self.show(&hit, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, key: Key, cx: &mut Context<Self>) {
        self.refusal = None;
        self.keyboard = false;
        self.cursor = Some(key.clone());
        if let Some(box_key) = box_key(&key) {
            self.store
                .update(cx, |store, cx| store.choose_box(box_key, cx));
            self.show(&key, cx);
        } else if let Some(id) = key.node() {
            self.store.update(cx, |store, cx| store.go(id, cx));
        }
        cx.notify();
    }

    fn show(&self, key: &Key, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| match key {
            Key::Node(id) | Key::Mark(id) => store.show_node(*id, cx),
            Key::Repo(repo) => store.show_repo(RepoId(*repo), cx),
            Key::Group(repo, path) => {
                store.choose_repo(RepoId(*repo), cx);
                store.show_boxes(vec![path.clone()], cx);
            }
        });
    }

    pub fn focus_find(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.field.read(cx).handle();
        window.focus(&handle, cx);
    }

    fn toggle(&mut self, key: &Key) {
        if !self.closed.remove(key) {
            self.closed.insert(key.clone());
        }
    }

    fn set_mark(&mut self, id: u32, on: bool, key: Key, cx: &mut Context<Self>) {
        let Some(base) = self.store.read(cx).base().map(str::to_owned) else {
            return;
        };
        self.refusal = None;
        cx.spawn(async move |this, cx| {
            let sent = cx
                .background_spawn(async move { link::set_bookmark(&base, id, on) })
                .await;
            if let Err(error) = sent {
                let _ = this.update(cx, |this, cx| {
                    this.refusal = Some((key, error.text(*cx.global::<Lang>())));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remove(
        &mut self,
        key: Key,
        work: impl FnOnce(&str) -> Result<(), crate::problem::Failure> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        calls::run(&store, cx, work, move |this, done, cx| {
            match done {
                Ok(()) => this.store.update(cx, |store, cx| store.reload(cx)),
                Err(text) => this.refusal = Some((key, text)),
            }
            cx.notify();
        });
    }

    fn act(&mut self, act: Act, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.refusal = None;
        match (act, &key) {
            (Act::Bookmark(id, on), _) => self.set_mark(id, !on, key, cx),
            (Act::Rename, _) => self.start_rename(key, window, cx),
            (Act::RemoveNode(id), _) => {
                self.remove(key, move |url| cardlink::remove_node(url, id), cx)
            }
            (Act::RemoveRepo, Key::Repo(id)) => {
                let id = *id;
                self.remove(key, move |url| calls::remove_repo(url, id), cx)
            }
            (Act::NewNode, _) => self.open_node(key, window, cx),
            (Act::NewWorktree, _) => self.open_worktree(key, window, cx),
            (Act::DeleteWorktree, _) => self.open_delete(key, window, cx),
            (Act::RemoveRepo, _) => {}
        }
    }

    fn row_of(&self, key: &Key, cx: &App) -> Option<Row> {
        let built = self.built(cx)?;
        built
            .marks
            .into_iter()
            .chain(built.tree)
            .find(|r| &r.key == key)
    }

    fn area(&self) -> Bounds<Pixels> {
        self.anchors.get(AREA).unwrap_or_default()
    }

    fn anchor(&self, key: &Key) -> Point<Pixels> {
        let area = self.area();
        let top = self
            .anchors
            .get(&format!("{key:?}"))
            .map_or(area.top() + px(ADD_TOP), |b| b.top());
        point(area.right() + px(POP_GAP), top)
    }

    fn close_pop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pop.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.area();
        let at = point(area.left() + px(ADD_LEFT), area.top() + px(ADD_TOP));
        let hosts = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| {
                s.repos
                    .iter()
                    .filter_map(|r| match &r.place {
                        PlaceView::Ssh { host } => Some(host.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let store = self.store.clone();
        let view = cx.new(|cx| AddRepo::new(store, hosts, at, window, cx));
        let closed = cx.subscribe_in(&view, window, |this, _, _: &Closed, window, cx| {
            this.close_pop(window, cx)
        });
        self.pop = Some(Open {
            view: view.into(),
            host: None,
            _closed: closed,
        });
        cx.notify();
    }

    fn host(
        &mut self,
        view: AnyView,
        at: Point<Pixels>,
        closed: Subscription,
        cx: &mut Context<Self>,
    ) {
        self.pop = Some(Open {
            view,
            host: Some(at),
            _closed: closed,
        });
        cx.notify();
    }

    fn open_worktree(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Key::Repo(repo) = key else { return };
        let Some(info) = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.repos.iter().find(|r| r.id.0 == repo))
            .cloned()
        else {
            return;
        };
        let at = self.anchor(&key);
        let store = self.store.clone();
        let view = cx.new(|cx| NewWorktreeView::new(store, info, window, cx));
        let closed = cx.subscribe_in(&view, window, |this, _, _: &Closed, window, cx| {
            this.close_pop(window, cx)
        });
        self.host(view.into(), at, closed, cx);
    }

    fn open_delete(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Row {
            body: Body::Group { label, .. },
            ..
        }) = self.row_of(&key, cx)
        else {
            return;
        };
        let Key::Group(repo, path) = &key else { return };
        let Some(snapshot) = self.store.read(cx).snapshot() else {
            return;
        };
        let doomed = Doomed::of(snapshot, RepoId(*repo), path.clone(), label);
        let at = self.anchor(&key);
        let store = self.store.clone();
        let view = cx.new(|cx| DeleteWorktreeView::new(store, doomed, window, cx));
        let closed = cx.subscribe_in(&view, window, |this, _, _: &Closed, window, cx| {
            this.close_pop(window, cx)
        });
        self.host(view.into(), at, closed, cx);
    }

    fn open_node(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Row {
            body: Body::Group { label, .. },
            ..
        }) = self.row_of(&key, cx)
        else {
            return;
        };
        let Key::Group(repo, path) = &key else { return };
        let place = Place {
            repo: RepoId(*repo),
            worktree: path.clone(),
        };
        let at = self.anchor(&key);
        let store = self.store.clone();
        let view = cx.new(|cx| NewNodeView::new(store, place, label, window, cx));
        let closed = cx.subscribe_in(&view, window, |this, _, _: &Closed, window, cx| {
            this.close_pop(window, cx)
        });
        self.host(view.into(), at, closed, cx);
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let modifiers = &stroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || self.renaming.is_some() {
            return;
        }
        let Some(built) = self.built(cx) else { return };
        self.keyboard = true;
        let rows: Vec<&Row> = built.rows(self.marks_open(cx)).collect();
        let searching = !self.field.read(cx).text().trim().is_empty();
        let at = rows
            .iter()
            .position(|r| Some(&r.key) == self.cursor.as_ref());
        let last = rows.len().saturating_sub(1);
        let target = match stroke.key.as_str() {
            "down" => Some(at.map_or(0, |i| (i + 1).min(last))),
            "up" => Some(at.map_or(0, |i| i.saturating_sub(1))),
            "home" => Some(0),
            "end" => Some(last),
            "right" => {
                let row = at.map(|i| rows[i]).filter(|r| r.branch);
                match row {
                    Some(row) if !row.open => {
                        self.closed.remove(&row.key);
                        None
                    }
                    Some(_) => at.map(|i| (i + 1).min(last)),
                    None => None,
                }
            }
            "left" => match at.map(|i| rows[i]) {
                Some(row) if row.branch && row.open && !searching => {
                    self.closed.insert(row.key.clone());
                    None
                }
                Some(row) => row
                    .parent
                    .as_ref()
                    .and_then(|p| rows.iter().position(|r| &r.key == p)),
                None => None,
            },
            "enter" | "space" => {
                if let Some(i) = at {
                    let key = rows[i].key.clone();
                    drop(rows);
                    self.activate(key, cx);
                    self.keyboard = true;
                }
                cx.stop_propagation();
                return;
            }
            "f2" => {
                let key = at
                    .map(|i| rows[i])
                    .filter(|r| acts::can_rename(r))
                    .map(|r| r.key.clone());
                drop(rows);
                if let Some(key) = key {
                    self.start_rename(key, window, cx);
                    cx.stop_propagation();
                }
                return;
            }
            "delete" => {
                let found = at
                    .map(|i| rows[i])
                    .and_then(|r| acts::remove_of(r).map(|act| (act, r.key.clone())));
                drop(rows);
                if let Some((act, key)) = found {
                    self.act(act, key, window, cx);
                    cx.stop_propagation();
                }
                return;
            }
            _ => return,
        };
        if let Some(i) = target.filter(|_| !rows.is_empty()) {
            self.cursor = Some(rows[i].key.clone());
            self.reveal = true;
        }
        cx.stop_propagation();
        cx.notify();
    }
}

fn box_key(key: &Key) -> Option<String> {
    match key {
        Key::Repo(repo) => Some(format!("repo:{repo}")),
        Key::Group(_, path) => Some(path.clone()),
        Key::Node(_) | Key::Mark(_) => None,
    }
}

fn dot_color(dot: Dot, c: &Colors) -> Rgba {
    match dot {
        Dot::Waiting => c.ask,
        Dot::Working => c.run,
        Dot::Idle => c.ink_3,
        Dot::Done => c.ok,
        Dot::Failed => c.err,
    }
}

pub fn dot(dot: Dot, c: &Colors) -> impl IntoElement {
    let core = div()
        .flex_none()
        .size(px(7.0))
        .rounded_full()
        .bg(dot_color(dot, c));
    if dot == Dot::Waiting {
        let mut ring = c.ask;
        ring.a = 0.25;
        div()
            .flex_none()
            .relative()
            .size(px(7.0))
            .child(
                div()
                    .absolute()
                    .top(px(-3.0))
                    .left(px(-3.0))
                    .size(px(13.0))
                    .rounded_full()
                    .bg(ring),
            )
            .child(core.absolute().top_0().left_0())
    } else {
        div().flex_none().size(px(7.0)).child(core)
    }
}

fn tag(tag: &Tag, id: SharedString, c: &Colors) -> impl IntoElement {
    let (fill, ink, edge) = match tag.tone {
        Tone::Ask => (Some(c.warn), c.warn_ink, c.warn),
        Tone::Done => (Some(c.ok), c.accent_ink, c.ok),
        Tone::Failed => (None, c.err, c.err),
        Tone::Neutral => (None, c.ink_2, c.ink_2),
    };
    let mut el = div()
        .id(id)
        .flex_none()
        .px(px(7.0))
        .h(px(18.0))
        .rounded(px(9.0))
        .border_1()
        .border_color(edge)
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .flex()
        .items_center()
        .child(tag.text.clone());
    if let Some(tip_text) = &tag.tip {
        el = el.tooltip(tip(tip_text.clone()));
    }
    if let Some(fill) = fill {
        el = el.bg(fill);
    }
    el
}

fn chevron(open: bool, c: &Colors) -> impl IntoElement {
    icon(
        if open {
            Lucide::ChevronDown
        } else {
            Lucide::ChevronRight
        },
        ICON,
        c.ink_3,
    )
}

fn ellipsed<E: Styled>(el: E) -> E {
    el.overflow_hidden().text_ellipsis().whitespace_nowrap()
}

fn named(text: &str, span: Option<(usize, usize)>, c: &Colors) -> AnyElement {
    match span {
        Some((from, to)) if to <= text.len() => {
            let mut wash = c.warn;
            wash.a = 0.3;
            StyledText::new(text.to_owned())
                .with_highlights(vec![(
                    from..to,
                    HighlightStyle {
                        background_color: Some(wash.into()),
                        ..Default::default()
                    },
                )])
                .into_any_element()
        }
        _ => text.to_owned().into_any_element(),
    }
}

fn fade(base: Rgba, wash: Option<Rgba>) -> gpui::Div {
    let across = |color: Rgba| {
        let mut clear = color;
        clear.a = 0.0;
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(linear_gradient(
                90.0,
                linear_color_stop(clear, 0.0),
                linear_color_stop(color, 1.0),
            ))
    };
    div()
        .relative()
        .flex_none()
        .w(px(FADE))
        .h(px(24.0))
        .child(across(base))
        .children(wash.map(across))
}

fn swap(name: Lucide, filled: bool, from: Rgba, to: Rgba, group: &SharedString) -> gpui::Div {
    div()
        .relative()
        .size(px(ICON))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .group_hover(group.clone(), |s| s.invisible())
                .child(icon_styled(name, ICON, from, 2.0, filled)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .invisible()
                .group_hover(group.clone(), |s| s.visible())
                .child(icon_styled(name, ICON, to, 2.0, filled)),
        )
}

impl Sidebar {
    fn strip(
        &self,
        key: &Key,
        buttons: Vec<Button>,
        chosen: bool,
        group: &str,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (base, wash) = if chosen {
            (c.panel, Some(c.accent_wash))
        } else {
            (c.surface_hover, None)
        };
        let items = buttons.into_iter().enumerate().map(|(n, button)| {
            let pad = SharedString::from(format!("{group}-act-{n}"));
            let on_ink = if button.pressed || !button.danger() {
                c.ink
            } else {
                c.err
            };
            let base_ink = if button.pressed { c.ink } else { c.ink_2 };
            let (act, row_key) = (button.act, key.clone());
            let line = c.line;
            let mut el = div()
                .id(pad.clone())
                .group(pad.clone())
                .size(px(24.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0));
            if button.disabled {
                el = el.opacity(0.4);
            } else {
                el = el
                    .cursor_pointer()
                    .hover(move |s| s.bg(line))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.act(act, row_key.clone(), window, cx);
                        cx.stop_propagation();
                    }));
            }
            el.tooltip(tip(button.label.clone())).child(swap(
                button.glyph,
                button.pressed,
                base_ink,
                on_ink,
                &pad,
            ))
        });
        let mut body = div().flex().gap(px(2.0)).bg(base);
        if let Some(wash) = wash {
            body = body.child(div().flex().gap(px(2.0)).bg(wash).children(items));
        } else {
            body = body.children(items);
        }
        div()
            .absolute()
            .top(px(3.0))
            .right(px(4.0))
            .h(px(24.0))
            .flex()
            .invisible()
            .group_hover(group.to_owned(), |s| s.visible())
            .child(fade(base, wash))
            .child(body)
    }

    fn row(
        &self,
        index: usize,
        section: &'static str,
        row: &Row,
        pick: &Pick,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = *cx.global::<Lang>();
        let key = row.key.clone();
        let depth = row.depth.min(MAX_DEPTH) as f32;
        let chosen = row.key.node().is_some() && row.key.node() == pick.selected;
        let tall = matches!(row.body, Body::Node { .. });
        let group = format!("{section}-{index}");
        let id = format!("{section}-{index}");
        let nudge = if tall { 3.0 } else { 0.0 };
        let editing = self.renaming.as_ref().filter(|r| r.key == row.key);
        let lead = if row.branch {
            let toggled = row.key.clone();
            div()
                .id((section, index + 100_000))
                .flex_none()
                .size(px(ICON))
                .mt(px(nudge))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle(&toggled);
                    this.keyboard = false;
                    this.cursor = Some(toggled.clone());
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(chevron(row.open, c))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .w(px(ICON))
                .mt(px(nudge))
                .into_any_element()
        };
        let lead = (!matches!(row.body, Body::Mark { .. })).then_some(lead);
        let edge = if pick.ring.as_ref() == Some(&row.key) {
            Some(c.accent)
        } else if row.found == Some(Found::Current) {
            Some(c.warn)
        } else {
            None
        };
        let mut shell = div()
            .id((section, index))
            .group(group.clone())
            .relative()
            .flex()
            .gap(px(6.0))
            .w_full()
            .h(px(if tall { NODE_ROW } else { ROW }))
            .pl(px(4.0 + depth * STEP))
            .pr(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.activate(key.clone(), cx);
            }))
            .children(lead);
        shell = if tall {
            shell.items_start().pt(px(4.0))
        } else {
            shell.items_center()
        };
        if chosen {
            shell = shell.bg(c.accent_wash);
        }
        let hover = c.surface_hover;
        shell = shell.group_hover(group.clone(), move |s| s.bg(hover));
        if row.found == Some(Found::Context) {
            shell = shell.opacity(0.55);
        }
        if let Some(edge) = edge {
            shell = shell.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(edge),
            );
        }
        if matches!(row.body, Body::Repo { .. } | Body::Group { .. }) {
            shell = shell.child(self.anchors.mark(format!("{:?}", row.key)));
        }
        let strip = if editing.is_some() {
            None
        } else {
            let buttons = acts::buttons(row, lang);
            (!buttons.is_empty()).then(|| self.strip(&row.key, buttons, chosen, &group, c, cx))
        };
        let tip_line = editing.map(|r| {
            let (text, ink) = match &r.error {
                Some(error) => (error.clone(), c.err),
                None => (Str::RenameHint.text(lang).to_owned(), c.ink_2),
            };
            gpui::deferred(
                div()
                    .absolute()
                    .top(px(if tall { NODE_ROW } else { ROW } + 4.0))
                    .left(px(8.0))
                    .right(px(8.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(c.line_strong)
                    .bg(c.surface)
                    .text_size(px(12.0))
                    .text_color(ink)
                    .child(text),
            )
        });
        let lang_unread = |n: u32| {
            div()
                .flex_none()
                .h(px(20.0))
                .px(px(7.0))
                .gap(px(4.0))
                .rounded(px(10.0))
                .bg(c.accent)
                .text_color(c.accent_ink)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .flex()
                .items_center()
                .child(icon(Lucide::Mail, 13.0, c.accent_ink))
                .child(n.to_string())
        };
        let shell = match &row.body {
            Body::Repo {
                name,
                path,
                full,
                chip,
                ..
            } => shell
                .child(
                    ellipsed(div().flex_none().max_w(relative(0.6)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(named(name, row.span, c)),
                )
                .children(chip.as_ref().map(|chip| {
                    div()
                        .id(SharedString::from(format!("{id}-place")))
                        .flex_none()
                        .px(px(6.0))
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .rounded(px(9.0))
                        .border_1()
                        .border_color(c.line_strong)
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.ink_2)
                        .whitespace_nowrap()
                        .tooltip(tip(chip.title.clone()))
                        .child(chip.text.clone())
                }))
                .child(
                    ellipsed(div().id(SharedString::from(format!("{id}-path"))).min_w_0())
                        .text_size(px(12.0))
                        .text_color(c.ink_3)
                        .tooltip(tip(full.clone()))
                        .child(path.clone()),
                ),
            Body::Group {
                label, tag, branch, ..
            } => {
                let path = match &row.key {
                    Key::Group(_, path) => path.clone(),
                    _ => String::new(),
                };
                let detail = if branch.is_some() {
                    format!("{label}\n{path}\n{}", Str::RenameTitle.text(lang))
                } else {
                    format!("{label}\n{path}")
                };
                let name = match editing {
                    Some(r) => self.editor(r, c).into_any_element(),
                    None => {
                        let mut el =
                            ellipsed(div().id(SharedString::from(format!("{id}-name"))).min_w_0())
                                .group(format!("{id}-name"))
                                .relative()
                                .cursor_text()
                                .font_weight(FontWeight::SEMIBOLD)
                                .tooltip(tip(detail))
                                .child(named(label, row.span, c))
                                .child(dotted::underline(
                                    format!("{id}-name"),
                                    c.ink_3,
                                    1.0,
                                    dotted::NAME_LIFT,
                                ));
                        if branch.is_some() {
                            el = el.flex_none().max_w(relative(0.6));
                            let key = row.key.clone();
                            el = el.on_click(cx.listener(
                                move |this, e: &ClickEvent, window, cx| {
                                    if e.click_count() >= 2 {
                                        this.start_rename(key.clone(), window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ));
                        }
                        el.into_any_element()
                    }
                };
                shell
                    .child(icon(Lucide::GitBranch, ICON, c.ink_3))
                    .child(name)
                    .children(tag.map(|text| {
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .text_color(c.ink_3)
                            .child(text)
                    }))
            }
            Body::Node {
                name,
                line,
                agent,
                agent_label,
                dot: state,
                tags,
                unread,
                from,
                ..
            } => {
                let crowded = *unread > 0 || !tags.is_empty();
                let name_el = match editing {
                    Some(r) => self.editor(r, c).into_any_element(),
                    None => {
                        let key = row.key.clone();
                        let base =
                            ellipsed(div().id(SharedString::from(format!("{id}-name"))).min_w_0())
                                .group(format!("{id}-name"))
                                .relative()
                                .cursor_text()
                                .font_weight(FontWeight::SEMIBOLD)
                                .tooltip(tip(Str::RenameTitle.text(lang).to_owned()))
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    if e.click_count() >= 2 {
                                        this.start_rename(key.clone(), window, cx);
                                        cx.stop_propagation();
                                    }
                                }))
                                .child(named(name, row.span, c))
                                .child(dotted::underline(
                                    format!("{id}-name"),
                                    c.ink_3,
                                    1.0,
                                    dotted::NAME_LIFT,
                                ));
                        if crowded {
                            base.flex_none().max_w(relative(0.6)).into_any_element()
                        } else {
                            base.into_any_element()
                        }
                    }
                };
                let first = div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(20.0))
                    .min_w_0()
                    .child(dot(*state, c))
                    .child(name_el)
                    .children(from.as_ref().map(|text| {
                        div()
                            .id(SharedString::from(format!("{id}-from")))
                            .flex_none()
                            .tooltip(tip(text.clone()))
                            .child(icon(Lucide::CornerLeftUp, 12.0, c.ink_3))
                    }))
                    .children((*unread > 0).then(|| div().flex_1()))
                    .children((*unread > 0).then(|| {
                        div()
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(lang_unread(*unread))
                    }))
                    .children((!tags.is_empty()).then(|| {
                        div().flex().flex_none().gap(px(4.0)).ml_auto().children(
                            tags.iter().enumerate().map(|(i, t)| {
                                tag(t, SharedString::from(format!("{id}-tag{i}")), c)
                            }),
                        )
                    }));
                let second = div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(16.0))
                    .ml(px(13.0))
                    .min_w_0()
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-agent")))
                            .flex_none()
                            .tooltip(tip(agent_label.clone()))
                            .child(agent_mark(agent.as_deref(), agent_label, ICON, c.ink_2)),
                    )
                    .child(
                        ellipsed(div().min_w_0())
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .child(fonts::latin_mono(line, c.ink_3, FontWeight::NORMAL)),
                    );
                shell.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(first)
                        .child(second),
                )
            }
            Body::Mark {
                name,
                place,
                dot: state,
                ..
            } => shell
                .child(dot(*state, c))
                .child(
                    ellipsed(div().flex_none().max_w(relative(0.6)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.clone()),
                )
                .child(
                    ellipsed(div().min_w_0())
                        .text_size(px(12.0))
                        .text_color(c.ink_3)
                        .child(place.clone()),
                ),
        };
        shell.children(strip).children(tip_line)
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.store.read(cx).settings().closed_panes.side {
            self.rail(cx).into_any_element()
        } else {
            if self.want_find {
                self.want_find = false;
                self.focus_find(window, cx);
            }
            if self.refocus {
                self.refocus = false;
                window.focus(&self.focus, cx);
            }
            self.load_agents(cx);
            self.expanded(window, cx).into_any_element()
        }
    }
}

impl Sidebar {
    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let lang = *cx.global::<Lang>();
        let finding = !self.field.read(cx).text().trim().is_empty();
        let open = rail::button(
            "side-open",
            Lucide::PanelLeftOpen,
            rail::titled(Str::AppSideExpand.text(lang), "["),
            rail::BUTTON,
            &c,
        )
        .on_click(cx.listener(|this, _, _, cx| this.set_open(true, cx)));
        let find = rail::button(
            "side-find",
            Lucide::Search,
            rail::titled(Str::FindOpen.text(lang), "F"),
            rail::BUTTON,
            &c,
        )
        .children(finding.then(|| rail::dot(&c)))
        .on_click(cx.listener(|this, _, _, cx| this.reveal_find(cx)));
        let settings = rail::button(
            "side-settings-rail",
            Lucide::Settings,
            Str::SettingsTitle.text(lang).to_owned(),
            rail::BUTTON,
            &c,
        )
        .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx));
        rail::column(&c)
            .child(open)
            .child(find)
            .child(div().flex_1())
            .child(settings)
    }

    fn set_open(&self, open: bool, cx: &mut Context<Self>) {
        self.store
            .update(cx, |store, cx| store.set_pane_open("side", open, cx));
    }

    pub fn reveal_find(&mut self, cx: &mut Context<Self>) {
        self.want_find = true;
        if self.store.read(cx).settings().closed_panes.side {
            self.set_open(true, cx);
        } else {
            cx.notify();
        }
    }

    fn expanded(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let c = &c;
        let lang = *cx.global::<Lang>();
        let built = self.built(cx);
        let selected = self.store.read(cx).selected();
        let focused = self.focus.is_focused(window);
        let pick = Pick {
            selected,
            ring: self.cursor.clone().filter(|_| focused && self.keyboard),
        };
        let query = self.field.read(cx).text().to_owned();
        let searching = !query.trim().is_empty();
        let hits = built.as_ref().map_or(0, |b| b.hits.len());
        let none = searching && hits == 0;
        let find_focused = self.field.read(cx).focused(window);

        let mut marks = Vec::new();
        let mut rows = Vec::new();
        if let Some(built) = &built {
            if self.reveal {
                self.reveal = false;
                let in_marks = built
                    .marks
                    .iter()
                    .position(|r| Some(&r.key) == self.cursor.as_ref());
                let in_tree = built
                    .tree
                    .iter()
                    .position(|r| Some(&r.key) == self.cursor.as_ref());
                if let Some(i) = in_marks {
                    self.marks_scroll.handle().scroll_to_item(i);
                }
                if let Some(i) = in_tree {
                    self.tree_scroll.handle().scroll_to_item(i);
                }
            }
            for (i, row) in built.marks.iter().enumerate() {
                marks.push((i, row, self.refusal_for(&row.key)));
            }
            for (i, row) in built.tree.iter().enumerate() {
                rows.push((i, row, self.refusal_for(&row.key)));
            }
        }
        let mark_count = built.as_ref().map_or(0, |b| b.marks.len());
        let marks_open = self.marks_open(cx);

        let find_edge = if none {
            c.err
        } else if find_focused {
            c.accent
        } else {
            c.line_strong
        };
        let count = if none {
            Some((Str::FindNone.text(lang).to_owned(), c.err, false))
        } else if hits > 0 {
            Some((format!("{}/{}", self.at + 1, hits), c.ink_3, true))
        } else {
            None
        };
        let clear = self.field.clone();
        let (title_hover, title_ink) = (c.surface_hover, c.ink);
        let head = || {
            div()
                .flex()
                .items_center()
                .pt(px(8.0))
                .pb(px(6.0))
                .pl(px(16.0))
                .pr(px(8.0))
                .text_size(px(12.0))
                .line_height(px(18.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c.ink_3)
        };

        let marks_head = head().h(px(35.0)).items_start().child(
            div()
                .id("marks-head")
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(4.0))
                .py(px(2.0))
                .mx(px(-4.0))
                .mt(px(-2.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(move |d| d.bg(title_hover).text_color(title_ink))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let open = this.marks_open(cx);
                    this.store
                        .update(cx, |store, cx| store.set_pane_open("bookmarks", !open, cx));
                }))
                .child(chevron(marks_open, c))
                .child(Str::BookmarkTitle.text(lang))
                .children((mark_count > 0).then(|| {
                    div()
                        .ml(px(2.0))
                        .font_weight(FontWeight::NORMAL)
                        .child(mark_count.to_string())
                })),
        );

        let mark_rows = div()
            .id("marks")
            .overflow_y_scroll()
            .track_scroll(self.marks_scroll.handle())
            .px(px(8.0))
            .flex()
            .flex_col()
            .children(marks.into_iter().map(|(i, row, refusal)| {
                let el = self.row(i, "mark", row, &pick, c, cx);
                div()
                    .child(el)
                    .children(refusal.map(|text| refusal_line(text, c)))
            }));
        let mark_rows = self
            .marks_scroll
            .reserved(mark_rows)
            .flex_none()
            .max_h(relative(0.4));

        let tree_rows = div()
            .id("tree")
            .overflow_y_scroll()
            .track_scroll(self.tree_scroll.handle())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)))
            .px(px(8.0))
            .flex()
            .flex_col()
            .children(rows.into_iter().map(|(i, row, refusal)| {
                let el = self.row(i, "tree", row, &pick, c, cx);
                div()
                    .child(el)
                    .children(refusal.map(|text| refusal_line(text, c)))
            }))
            .children(none.then(|| {
                div()
                    .px(px(8.0))
                    .py(px(8.0))
                    .text_size(px(12.0))
                    .text_color(c.ink_3)
                    .child(Str::FindNoMatch.text(lang))
            }));
        let tree_rows = self.tree_scroll.reserved(tree_rows).flex_1().min_h_0();

        let tool = |name: &'static str, glyph: Lucide, label: String| {
            let hover = c.surface_hover;
            div()
                .id(name)
                .w(px(32.0))
                .h(px(rail::SMALL_BUTTON))
                .flex_none()
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(tip(label))
                .child(icon(glyph, ICON, c.ink_2))
        };
        let add = tool(
            "add-repo",
            Lucide::Plus,
            Str::TreeAddRepo.text(lang).to_owned(),
        )
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_add(window, cx)));
        let expand = tool(
            "expand-all",
            Lucide::ChevronsUpDown,
            Str::TreeExpandAll.text(lang).to_owned(),
        )
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.closed.clear();
            cx.notify();
        }));
        let overlays: Vec<AnyElement> = self
            .pop
            .iter()
            .map(|open| match open.host {
                Some(at) => pop::overlay(
                    at,
                    open.view.clone(),
                    cx.listener(|this, _, window, cx| this.close_pop(window, cx)),
                    window,
                )
                .into_any_element(),
                None => open.view.clone().into_any_element(),
            })
            .collect();

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.panel)
            .text_color(c.ink)
            .text_size(px(13.0))
            .border_r_1()
            .border_color(c.line)
            .child(self.anchors.mark(AREA.to_owned()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pt(px(12.0))
                    .px(px(12.0))
                    .pb(px(4.0))
                    .child(
                        div()
                            .id("find-box")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(30.0))
                            .pl(px(8.0))
                            .pr(px(2.0))
                            .bg(c.bg)
                            .border_1()
                            .border_color(find_edge)
                            .rounded(px(6.0))
                            .tooltip(tip(rail::titled(Str::FindOpen.text(lang), "F")))
                            .child(icon(Lucide::Search, ICON, c.ink_3))
                            .child(self.field.clone())
                            .children(count.map(|(text, color, mono)| {
                                let el = div().flex_none().text_size(px(12.0)).text_color(color);
                                if mono {
                                    el.font_family(fonts::TERMINAL_FAMILY).child(text)
                                } else {
                                    el.child(text)
                                }
                            }))
                            .children(searching.then(|| {
                                let hover = c.surface_hover;
                                div()
                                    .id("find-clear")
                                    .flex_none()
                                    .size(px(26.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover))
                                    .tooltip(tip(format!("{} (Esc)", Str::FindClear.text(lang))))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(move |_, _, cx| {
                                        clear.update(cx, |field, cx| field.clear(cx));
                                    })
                                    .child(icon(Lucide::X, ICON, c.ink_2))
                            })),
                    )
                    .child(
                        rail::button(
                            "side-collapse",
                            Lucide::PanelLeftClose,
                            rail::titled(Str::AppSideCollapse.text(lang), "["),
                            rail::SMALL_BUTTON,
                            c,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.set_open(false, cx))),
                    ),
            )
            .child(marks_head)
            .children(marks_open.then(|| {
                if mark_count == 0 {
                    div()
                        .px(px(16.0))
                        .pb(px(6.0))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(c.ink_3)
                        .child(Str::BookmarkEmpty.text(lang))
                        .into_any_element()
                } else {
                    mark_rows.into_any_element()
                }
            }))
            .child(
                head()
                    .child(
                        div()
                            .id("side-title")
                            .px(px(4.0))
                            .py(px(2.0))
                            .mx(px(-4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(move |d| d.bg(title_hover).text_color(title_ink))
                            .tooltip(tip(Str::TreeShowAll.text(lang).to_owned()))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.store.update(cx, |store, cx| {
                                    store.clear_selection(cx);
                                    store.show_all(cx);
                                });
                            }))
                            .child(Str::TreeRepos.text(lang)),
                    )
                    .child(div().flex_1())
                    .child(add)
                    .child(expand),
            )
            .child(tree_rows)
            .child(
                div()
                    .flex_none()
                    .p(px(8.0))
                    .border_t_1()
                    .border_color(c.line)
                    .child(
                        div()
                            .id("side-settings")
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(12.0))
                            .h(px(30.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(c.ink_2)
                            .cursor_pointer()
                            .hover({
                                let hover = c.surface_hover;
                                let ink = c.ink;
                                move |s| s.bg(hover).text_color(ink)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenSettings), cx)
                            })
                            .child(icon(Lucide::Settings, ICON, c.ink_2))
                            .child(Str::SettingsTitle.text(lang)),
                    ),
            )
            .children(overlays)
    }
}

impl Sidebar {
    fn refusal_for(&self, key: &Key) -> Option<String> {
        self.refusal
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, text)| text.clone())
    }
}

fn refusal_line(text: String, c: &Colors) -> impl IntoElement {
    div()
        .mx(px(8.0))
        .mt(px(2.0))
        .mb(px(4.0))
        .ml(px(26.0))
        .text_size(px(12.0))
        .text_color(c.err)
        .child(text)
}
