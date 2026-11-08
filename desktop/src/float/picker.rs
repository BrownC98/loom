use std::cell::Cell;
use std::rc::Rc;

use api::graph::NodeState;
use api::graph::search::WaitSearch;
use api::graph::waits::{WaitKind, WaitTone};
use api::ids::{NodeId, WaitId};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render,
    ScrollEvent, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::aim::{self, Aim};
use super::place;
use crate::store::Store;
use crate::strings::Str;
use crate::theme::Colors;
use crate::views::dock::entry;
use crate::views::dock::questions::kind_label;
use crate::views::dock::ui;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};
use crate::views::scrollbar::Bar;

const SHADOW_BLUR: f32 = 14.0;
const SHADOW_DROP: f32 = 4.0;
const TIME: &str = "%H:%M";
const HINT_ROOM: f32 = 480.0;

pub enum PickerEvent {
    Pick(NodeId, Aim),
    Close,
    Escape,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chip {
    All,
    Ask,
    Done,
}

enum Item {
    Alert {
        id: WaitId,
        kind: WaitKind,
        tone: WaitTone,
        text: String,
        since: i64,
    },
    Prompt,
}

impl Item {
    fn aim(&self) -> Aim {
        match self {
            Item::Alert { id, .. } => Aim::Alert(*id),
            Item::Prompt => Aim::Prompt,
        }
    }
}

enum Head {
    Title(Str),
    Repo(String),
}

struct Row {
    node: NodeId,
    name: String,
    item: Item,
}

struct Section {
    head: Head,
    rows: Vec<Row>,
}

struct Listing {
    sections: Vec<Section>,
    all: usize,
    asks: usize,
    dones: usize,
}

impl Listing {
    fn flat(&self) -> Vec<(NodeId, Aim)> {
        self.sections
            .iter()
            .flat_map(|s| s.rows.iter().map(|row| (row.node, row.item.aim())))
            .collect()
    }

    fn child_of(&self, at: usize) -> usize {
        let mut before = 0;
        let mut heads = 0;
        for section in &self.sections {
            heads += 1;
            if at < before + section.rows.len() {
                break;
            }
            before += section.rows.len();
        }
        heads + at
    }
}

pub struct Picker {
    store: Entity<Store>,
    current: (NodeId, Aim),
    query: Entity<Field>,
    chip: Chip,
    at: usize,
    bar: Bar,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    zoom: f32,
    width: f32,
    room: f32,
    above: bool,
    placed: bool,
    _subscription: Subscription,
}

impl EventEmitter<PickerEvent> for Picker {}

pub fn picker(
    store: Entity<Store>,
    current: (NodeId, Aim),
    anchor: Rc<Cell<Bounds<Pixels>>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Picker> {
    let query = field::new(Str::InboxSearchPlaceholder, cx);
    let handle = query.read(cx).handle();
    window.focus(&handle, cx);
    cx.new(|cx| {
        let subscription = cx.subscribe(
            &query,
            |picker: &mut Picker, _, event: &FieldEvent, cx| match event {
                FieldEvent::Changed => picker.restart(cx),
                FieldEvent::Step(by) => picker.step(*by, cx),
                FieldEvent::Enter => {
                    if let Some((node, aim)) = picker.listing(cx).flat().get(picker.at).copied() {
                        cx.emit(PickerEvent::Pick(node, aim));
                    }
                }
                FieldEvent::Escape => cx.emit(PickerEvent::Escape),
            },
        );
        Picker {
            store,
            current,
            query,
            chip: Chip::All,
            at: 0,
            bar: Bar::new(),
            anchor,
            zoom: 1.0,
            width: 0.0,
            room: 0.0,
            above: false,
            placed: false,
            _subscription: subscription,
        }
    })
}

fn live(state: &NodeState) -> bool {
    matches!(state, NodeState::Running | NodeState::Done)
}

impl Picker {
    pub fn place(&mut self, zoom: f32, width: f32, room: f32, above: bool, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.width = width;
        self.room = room;
        self.above = above;
        if !self.placed {
            self.placed = true;
            self.at = self.start(cx);
            self.reveal(cx);
        }
    }

    fn listing(&self, cx: &App) -> Listing {
        let store = self.store.read(cx);
        let Some(snapshot) = store.snapshot() else {
            return Listing {
                sections: Vec::new(),
                all: 0,
                asks: 0,
                dones: 0,
            };
        };
        let search = WaitSearch::new(self.query.read(cx).text());
        let mut alerts = aim::stream_of(snapshot);
        if let Some(search) = &search {
            alerts.retain(|s| search.finds_open(&s.name, &s.wait.wait));
        }
        let asks = alerts
            .iter()
            .filter(|s| matches!(s.wait.tone, WaitTone::Ask))
            .count();
        let all = alerts.len();
        alerts.retain(|s| match self.chip {
            Chip::All => true,
            Chip::Ask => matches!(s.wait.tone, WaitTone::Ask),
            Chip::Done => matches!(s.wait.tone, WaitTone::Done),
        });
        let mut sections = Vec::new();
        let shown = |n: &&api::graph::NodeView| {
            (live(&n.state) || n.id == self.current.0)
                && search
                    .as_ref()
                    .is_none_or(|s| n.name.to_lowercase().contains(s.words()))
        };
        let mut nodes = Vec::new();
        for repo in &snapshot.repos {
            let rows: Vec<Row> = snapshot
                .nodes
                .iter()
                .filter(shown)
                .filter(|n| n.place.repo == repo.id)
                .map(|n| Row {
                    node: n.id,
                    name: n.name.clone(),
                    item: Item::Prompt,
                })
                .collect();
            if !rows.is_empty() {
                nodes.push(Section {
                    head: Head::Repo(repo.name.clone()),
                    rows,
                });
            }
        }
        if !nodes.is_empty() {
            sections.push(Section {
                head: Head::Title(Str::ComposerSectionNodes),
                rows: Vec::new(),
            });
            sections.extend(nodes);
        }
        if !alerts.is_empty() {
            sections.push(Section {
                head: Head::Title(Str::ComposerSectionAlerts),
                rows: alerts
                    .into_iter()
                    .map(|s| Row {
                        node: NodeId(s.node),
                        name: s.name,
                        item: Item::Alert {
                            id: s.wait.wait.id,
                            kind: s.wait.wait.kind,
                            tone: s.wait.tone,
                            text: s.wait.summary,
                            since: s.wait.wait.since,
                        },
                    })
                    .collect(),
            });
        }
        Listing {
            sections,
            all,
            asks,
            dones: all - asks,
        }
    }

    fn start(&self, cx: &App) -> usize {
        self.listing(cx)
            .flat()
            .iter()
            .position(|row| *row == self.current)
            .unwrap_or(0)
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.at = 0;
        self.reveal(cx);
    }

    fn reveal(&mut self, cx: &mut Context<Self>) {
        let child = self.listing(cx).child_of(self.at);
        self.bar.reset();
        self.bar.handle().scroll_to_item(child);
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let listing = self.listing(cx);
        let count = listing.flat().len() as isize;
        if count == 0 {
            return;
        }
        self.at = (self.at as isize + by).rem_euclid(count) as usize;
        self.bar.handle().scroll_to_item(listing.child_of(self.at));
        cx.notify();
    }

    fn choose(&mut self, chip: Chip, cx: &mut Context<Self>) {
        self.chip = chip;
        self.restart(cx);
    }

    fn chip(
        &self,
        chip: Chip,
        label: Str,
        count: usize,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let z = self.zoom;
        let on = self.chip == chip;
        let (fill, ink) = (c.accent, c.accent_ink);
        let hover = c.surface_hover;
        div()
            .id(match chip {
                Chip::All => "composer-chip-all",
                Chip::Ask => "composer-chip-ask",
                Chip::Done => "composer-chip-done",
            })
            .flex_none()
            .flex()
            .items_center()
            .h(px(22.0 * z))
            .px(px(8.0 * z))
            .rounded(px(11.0 * z))
            .border_1()
            .text_size(px(12.0 * z))
            .cursor_pointer()
            .when(on, |d| {
                d.bg(fill)
                    .border_color(fill)
                    .text_color(ink)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!on, |d| {
                d.border_color(c.line_strong)
                    .text_color(c.ink_2)
                    .hover(move |s| s.bg(hover))
            })
            .on_click(cx.listener(move |picker, _, _, cx| picker.choose(chip, cx)))
            .child(ui::say_with(cx, label, &[("count", &count.to_string())]))
    }

    fn row(&self, n: usize, row: &Row, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let z = self.zoom;
        let (node, aim) = (row.node, row.item.aim());
        let current = (node, aim) == self.current;
        let hover = c.surface_hover;
        let base = div()
            .id(("composer-pick-row", n))
            .flex()
            .items_center()
            .gap(px(8.0 * z))
            .min_h(px(26.0 * z))
            .px(px(8.0 * z))
            .rounded(px(6.0 * z))
            .cursor_pointer()
            .when(n == self.at, |d| d.bg(hover))
            .when(current, |d| d.bg(c.accent_wash))
            .on_mouse_move(cx.listener(move |picker, _: &MouseMoveEvent, _, cx| {
                if picker.at != n {
                    picker.at = n;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Pick(node, aim))));
        match &row.item {
            Item::Alert {
                id,
                kind,
                tone,
                text,
                since,
            } => base
                .child(entry::mark(*tone, z, c))
                .child(entry::qid(
                    if *kind == WaitKind::Question {
                        id.to_string()
                    } else {
                        ui::say(cx, kind_label(*kind)).to_owned()
                    },
                    *tone,
                    18.0,
                    z,
                    c,
                ))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(140.0 * z))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(c.ink_2)
                        .child(text.replace('\n', " ")),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0 * z))
                        .text_color(c.ink_3)
                        .child(ui::clock(*since, TIME)),
                ),
            Item::Prompt => base
                .text_color(c.ink_2)
                .child(icon(Lucide::SquareTerminal, 14.0 * z, c.ink_3))
                .child(div().min_w_0().truncate().child(ui::say_with(
                    cx,
                    Str::ComposerInstruct,
                    &[("name", &row.name)],
                ))),
        }
    }

    fn head(&self, head: &Head, c: &Colors, cx: &App) -> impl IntoElement {
        let z = self.zoom;
        let line = div()
            .flex()
            .items_center()
            .gap(px(6.0 * z))
            .px(px(8.0 * z))
            .pt(px(6.0 * z))
            .pb(px(2.0 * z))
            .text_size(px(12.0 * z));
        match head {
            Head::Title(label) => line
                .pt(px(8.0 * z))
                .text_color(c.ink)
                .font_weight(FontWeight::SEMIBOLD)
                .child(ui::say(cx, *label)),
            Head::Repo(name) => line
                .text_color(c.ink_3)
                .child(div().min_w_0().truncate().child(name.clone())),
        }
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ui::colors(cx);
        let z = self.zoom;
        let listing = self.listing(cx);
        let anchor = self.anchor.clone();
        let mut list = div()
            .id("composer-pick-list")
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(self.bar.handle())
            .px(px(6.0 * z))
            .pb(px(6.0 * z));
        let mut n = 0;
        for section in &listing.sections {
            list = list.child(self.head(&section.head, &c, cx));
            for row in &section.rows {
                list = list.child(self.row(n, row, &c, cx));
                n += 1;
            }
        }
        let chips = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0 * z))
            .child(self.chip(Chip::All, Str::ComposerChipAll, listing.all, &c, cx))
            .child(self.chip(Chip::Ask, Str::ComposerChipQuestion, listing.asks, &c, cx))
            .child(self.chip(Chip::Done, Str::ComposerChipDone, listing.dones, &c, cx));
        let find = div()
            .flex()
            .items_center()
            .gap(px(8.0 * z))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0 * z))
                    .px(px(8.0 * z))
                    .h(px(28.0 * z))
                    .rounded(px(6.0 * z))
                    .border_1()
                    .border_color(c.accent)
                    .bg(c.surface)
                    .text_size(px(13.0 * z))
                    .child(icon(Lucide::Search, 14.0 * z, c.ink_3))
                    .child(self.query.clone())
                    .children((self.width >= HINT_ROOM * z).then(|| {
                        div()
                            .flex_none()
                            .text_size(px(11.0 * z))
                            .text_color(c.ink_3)
                            .whitespace_nowrap()
                            .child(ui::say(cx, Str::ComposerListHint))
                    })),
            )
            .child(chips);
        let foot = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0 * z))
            .px(px(10.0 * z))
            .py(px(8.0 * z))
            .border_color(c.line)
            .when(self.above, |d| d.border_t_1())
            .when(!self.above, |d| d.border_b_1())
            .child(find);
        let body = div()
            .id("composer-pick")
            .w(px(self.width))
            .max_h(px(self.room))
            .flex()
            .when(self.above, |d| d.flex_col_reverse())
            .when(!self.above, |d| d.flex_col())
            .overflow_hidden()
            .rounded(px(10.0 * z))
            .border_1()
            .border_color(c.line_strong)
            .bg(c.panel)
            .text_color(c.ink)
            .text_size(px(13.0 * z))
            .shadow(vec![gpui::BoxShadow {
                color: c.shadow.into(),
                offset: gpui::point(px(0.0), px(SHADOW_DROP)),
                blur_radius: px(SHADOW_BLUR),
                spread_radius: px(0.0),
                inset: false,
            }])
            .occlude()
            .on_mouse_down(MouseButton::Left, place::stop)
            .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |_, e: &MouseDownEvent, _, cx| {
                if !anchor.get().contains(&e.position) {
                    cx.emit(PickerEvent::Close);
                }
            }))
            .child(foot);
        if n == 0 {
            body.child(
                div()
                    .px(px(14.0 * z))
                    .py(px(10.0 * z))
                    .text_color(c.ink_3)
                    .child(ui::say(cx, Str::InboxNoMatch)),
            )
        } else {
            body.child(self.bar.reserved(list).min_h_0())
        }
    }
}
