pub mod icons;
mod tree;

use std::collections::HashSet;

use gpui::{
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, Rgba, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, px, relative,
};

use super::field::{self, Field, FieldEvent};
use super::shell::OpenSettings;
use icons::{Glyph, icon};
use tree::{Body, Built, Dot, Found, Key, Row, Tag, Tone};

use crate::fonts;
use crate::link;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};

const STEP: f32 = 12.0;
const MAX_DEPTH: usize = 8;
const ROW: f32 = 30.0;
const NODE_ROW: f32 = 44.0;
const ICON: f32 = 14.0;

struct Pick {
    selected: Option<u32>,
    ring: Option<Key>,
}

pub struct Sidebar {
    store: Entity<Store>,
    field: Entity<Field>,
    focus: FocusHandle,
    marks_scroll: ScrollHandle,
    tree_scroll: ScrollHandle,
    closed: HashSet<Key>,
    marks_open: bool,
    cursor: Option<Key>,
    reveal: bool,
    seen: Option<u32>,
    at: usize,
    refusal: Option<(Key, String)>,
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
            marks_scroll: ScrollHandle::new(),
            tree_scroll: ScrollHandle::new(),
            closed: HashSet::new(),
            marks_open: true,
            cursor: None,
            reveal: false,
            seen: None,
            at: 0,
            refusal: None,
        }
    }

    fn built(&self, cx: &App) -> Option<Built> {
        let snapshot = self.store.read(cx).snapshot()?;
        Some(tree::build(
            snapshot,
            *cx.global::<Lang>(),
            &self.closed,
            self.field.read(cx).text(),
            self.at,
        ))
    }

    fn find(&mut self, event: &FieldEvent, cx: &mut Context<Self>) {
        match event {
            FieldEvent::Changed => {
                self.at = 0;
                self.cursor = self.built(cx).and_then(|b| b.hits.first().cloned());
                self.reveal = true;
            }
            FieldEvent::Step(step) => {
                let Some(built) = self.built(cx) else { return };
                if !built.hits.is_empty() {
                    let len = built.hits.len() as isize;
                    self.at = (self.at as isize + step).rem_euclid(len) as usize;
                    self.cursor = built.hits.get(self.at).cloned();
                    self.reveal = true;
                }
            }
            FieldEvent::Enter => {
                let Some(built) = self.built(cx) else { return };
                if let Some(Key::Node(id)) = built.hits.get(self.at) {
                    let id = *id;
                    self.store.update(cx, |store, cx| store.select(id, cx));
                }
            }
            FieldEvent::Escape => {
                self.field.update(cx, |field, cx| field.clear(cx));
                self.reveal = true;
            }
        }
        let found = self.built(cx).filter(|b| !b.hits.is_empty()).map(|b| {
            b.hits
                .iter()
                .map(|key| match key {
                    Key::Node(id) | Key::Mark(id) => id.to_string(),
                    Key::Repo(id) => format!("repo:{id}"),
                    Key::Group(_, path) => path.clone(),
                })
                .collect()
        });
        self.store
            .update(cx, |store, cx| store.set_found(found, cx));
        cx.notify();
    }

    fn activate(&mut self, key: Key, cx: &mut Context<Self>) {
        self.refusal = None;
        self.cursor = Some(key.clone());
        match key {
            Key::Node(id) | Key::Mark(id) => {
                self.store.update(cx, |store, cx| store.select(id, cx));
            }
            Key::Repo(_) | Key::Group(..) => self.toggle(&key),
        }
        cx.notify();
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

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let modifiers = &stroke.modifiers;
        if modifiers.control || modifiers.platform {
            if stroke.key == "f" {
                self.focus_find(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if modifiers.alt {
            return;
        }
        let Some(built) = self.built(cx) else { return };
        let rows: Vec<&Row> = built.rows(self.marks_open).collect();
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
                }
                cx.stop_propagation();
                return;
            }
            "/" => {
                self.focus_find(window, cx);
                cx.stop_propagation();
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

fn dot_color(dot: Dot, c: &Colors) -> Rgba {
    match dot {
        Dot::Waiting => c.ask,
        Dot::Running => c.run,
        Dot::Done => c.ok,
        Dot::Failed => c.err,
    }
}

fn dot(dot: Dot, c: &Colors) -> impl IntoElement {
    let color = dot_color(dot, c);
    let halo = div().flex_none().size(px(7.0)).rounded_full().bg(color);
    if dot == Dot::Waiting {
        let mut ring = c.ask;
        ring.a = 0.25;
        div()
            .flex_none()
            .size(px(13.0))
            .rounded_full()
            .bg(ring)
            .flex()
            .items_center()
            .justify_center()
            .child(halo)
    } else {
        div()
            .flex_none()
            .size(px(13.0))
            .flex()
            .items_center()
            .justify_center()
            .child(halo)
    }
}

fn tag(tag: &Tag, c: &Colors) -> impl IntoElement {
    let (fill, ink, edge) = match tag.tone {
        Tone::Ask => (Some(c.warn), c.warn_ink, c.warn),
        Tone::Done => (Some(c.ok), c.accent_ink, c.ok),
        Tone::Failed => (None, c.err, c.err),
    };
    let mut el = div()
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
    if let Some(fill) = fill {
        el = el.bg(fill);
    }
    el
}

fn chevron(open: bool, c: &Colors) -> impl IntoElement {
    icon(
        if open {
            Glyph::ChevronOpen
        } else {
            Glyph::ChevronClosed
        },
        ICON,
        c.ink_3,
    )
}

impl Sidebar {
    fn row(
        &self,
        index: usize,
        section: &'static str,
        row: &Row,
        pick: &Pick,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = row.key.clone();
        let depth = row.depth.min(MAX_DEPTH) as f32;
        let chosen = row.key.node().is_some() && row.key.node() == pick.selected;
        let tall = matches!(row.body, Body::Node { .. });
        let group = format!("{section}-{index}");
        let nudge = if tall { 3.0 } else { 0.0 };
        let lead = if row.branch {
            let toggled = row.key.clone();
            div()
                .id((section, index + 100_000))
                .flex_none()
                .size(px(ICON))
                .mt(px(nudge))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle(&toggled);
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
        let edge = if pick.ring.as_ref() == Some(&row.key) {
            c.accent
        } else if row.found == Some(Found::Current) {
            c.ask
        } else {
            Rgba::default()
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
            .border_2()
            .border_color(edge)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.activate(key.clone(), cx);
            }))
            .child(lead);
        shell = if tall {
            shell.items_start().pt(px(2.0))
        } else {
            shell.items_center()
        };
        shell = if chosen {
            shell.bg(c.accent_wash)
        } else {
            let hover = c.surface_hover;
            shell.hover(move |s| s.bg(hover))
        };
        if row.found == Some(Found::Context) {
            shell = shell.opacity(0.55);
        }
        let marked_toggle = |id: u32, on: bool, row_key: Key, cx: &mut Context<Self>| {
            let group = group.clone();
            let hover = c.surface_hover;
            div()
                .id((section, index + 200_000))
                .absolute()
                .top(px(if tall { 6.0 } else { 2.0 }))
                .right(px(4.0))
                .size(px(24.0))
                .rounded(px(5.0))
                .bg(hover)
                .flex()
                .items_center()
                .justify_center()
                .invisible()
                .group_hover(group, |s| s.visible())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.set_mark(id, !on, row_key.clone(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(icon(Glyph::Bookmark { filled: on }, ICON, c.ink_2))
        };
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
                .child(icon(Glyph::Mail, 13.0, c.accent_ink))
                .child(n.to_string())
        };
        match &row.body {
            Body::Repo { name, path } => shell
                .child(
                    div()
                        .flex_none()
                        .max_w(relative(0.6))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(c.ink_3)
                        .child(path.clone()),
                )
                .into_any_element(),
            Body::Group { label, tag } => shell
                .child(icon(Glyph::Branch, ICON, c.ink_3))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label.clone()),
                )
                .children(tag.map(|text| {
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(c.ink_3)
                        .child(text)
                }))
                .into_any_element(),
            Body::Node {
                id,
                name,
                line,
                dot: state,
                tags,
                unread,
                from,
                marked,
            } => {
                let first = div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(20.0))
                    .min_w_0()
                    .child(dot(*state, c))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.clone()),
                    )
                    .children(from.as_ref().map(|_| icon(Glyph::Reply, 12.0, c.ink_3)))
                    .child(div().flex_1())
                    .children((*unread > 0).then(|| {
                        div()
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(lang_unread(*unread))
                    }))
                    .children(tags.iter().map(|t| tag(t, c)));
                let second = div()
                    .h(px(16.0))
                    .ml(px(13.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.0))
                    .font_family(fonts::TERMINAL_FAMILY)
                    .text_color(c.ink_3)
                    .child(line.clone());
                shell
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(first)
                            .child(second),
                    )
                    .child(marked_toggle(*id, *marked, row.key.clone(), cx))
                    .into_any_element()
            }
            Body::Mark {
                id,
                name,
                place,
                dot: state,
            } => shell
                .child(dot(*state, c))
                .child(
                    div()
                        .flex_none()
                        .max_w(relative(0.6))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(c.ink_3)
                        .child(place.clone()),
                )
                .child(marked_toggle(*id, true, row.key.clone(), cx))
                .into_any_element(),
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let c = &c;
        let lang = *cx.global::<Lang>();
        let built = self.built(cx);
        let selected = self.store.read(cx).selected();
        let focused = self.focus.is_focused(window);
        let pick = Pick {
            selected,
            ring: self.cursor.clone().filter(|_| focused),
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
                    self.marks_scroll.scroll_to_item(i);
                }
                if let Some(i) = in_tree {
                    self.tree_scroll.scroll_to_item(i);
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

        let find_edge = if none {
            c.err
        } else if find_focused {
            c.accent
        } else {
            c.line_strong
        };
        let count = if none {
            Some((Str::FindNone.text(lang).to_owned(), c.err))
        } else if hits > 0 {
            Some((format!("{}/{}", self.at + 1, hits), c.ink_3))
        } else {
            None
        };
        let clear = self.field.clone();
        let head = || {
            div()
                .flex()
                .items_center()
                .h(px(28.0))
                .pl(px(16.0))
                .pr(px(8.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c.ink_3)
        };

        let marks_head = head().child(
            div()
                .id("marks-head")
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.marks_open = !this.marks_open;
                    cx.notify();
                }))
                .child(chevron(self.marks_open, c))
                .child(Str::BookmarkTitle.text(lang))
                .children((mark_count > 0).then(|| {
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .child(mark_count.to_string())
                })),
        );

        let mark_rows = div()
            .id("marks")
            .flex_none()
            .max_h(relative(0.4))
            .overflow_y_scroll()
            .track_scroll(&self.marks_scroll)
            .px(px(8.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .children(marks.into_iter().map(|(i, row, refusal)| {
                let el = self.row(i, "mark", row, &pick, c, cx);
                div()
                    .child(el)
                    .children(refusal.map(|text| refusal_line(text, c)))
            }));

        let tree_rows = div()
            .id("tree")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.tree_scroll)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)))
            .px(px(8.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
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

        let expand = div()
            .id("expand-all")
            .size(px(26.0))
            .rounded(px(5.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover({
                let hover = c.surface_hover;
                move |s| s.bg(hover)
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.closed.clear();
                cx.notify();
            }))
            .child(icon(Glyph::ExpandAll, ICON, c.ink_2));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.panel)
            .text_color(c.ink)
            .text_size(px(13.0))
            .border_r_1()
            .border_color(c.line)
            .child(
                div().px(px(10.0)).pt(px(10.0)).pb(px(6.0)).child(
                    div()
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
                        .child(icon(Glyph::Search, ICON, c.ink_3))
                        .child(self.field.clone())
                        .children(count.map(|(text, color)| {
                            div()
                                .flex_none()
                                .text_size(px(12.0))
                                .text_color(color)
                                .child(text)
                        }))
                        .children(searching.then(|| {
                            div()
                                .id("find-clear")
                                .flex_none()
                                .size(px(26.0))
                                .rounded(px(5.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    clear.update(cx, |field, cx| field.clear(cx));
                                })
                                .child(icon(Glyph::Clear, ICON, c.ink_2))
                        })),
                ),
            )
            .child(marks_head)
            .children(self.marks_open.then(|| {
                if mark_count == 0 {
                    div()
                        .px(px(16.0))
                        .pb(px(6.0))
                        .text_size(px(12.0))
                        .text_color(c.ink_3)
                        .child(Str::BookmarkEmpty.text(lang))
                        .into_any_element()
                } else {
                    mark_rows.into_any_element()
                }
            }))
            .child(
                head()
                    .child(Str::TreeRepos.text(lang))
                    .child(div().flex_1())
                    .child(expand),
            )
            .child(tree_rows)
            .child(
                div()
                    .flex_none()
                    .px(px(8.0))
                    .py(px(6.0))
                    .border_t_1()
                    .border_color(c.line)
                    .child(
                        div()
                            .id("side-settings")
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(8.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .text_size(px(12.0))
                            .text_color(c.ink_2)
                            .cursor_pointer()
                            .hover({
                                let hover = c.surface_hover;
                                move |s| s.bg(hover)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenSettings), cx)
                            })
                            .child(icon(Glyph::Gear, ICON, c.ink_2))
                            .child(Str::SettingsTitle.text(lang)),
                    ),
            )
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
