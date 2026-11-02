use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render,
    ScrollEvent, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::place;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::views::dock::ui;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};
use crate::views::scrollbar::Bar;
use crate::views::sidebar::{Pickable, dot, pickables};

const SHADOW_BLUR: f32 = 14.0;
const SHADOW_DROP: f32 = 4.0;

pub enum PickerEvent {
    Pick(u32),
    Close,
    Escape,
}

pub struct Picker {
    store: Entity<Store>,
    current: u32,
    query: Entity<Field>,
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
    current: u32,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Picker> {
    let query = field::new(Str::ComposerPickSearch, cx);
    let handle = query.read(cx).handle();
    window.focus(&handle, cx);
    cx.new(|cx| {
        let subscription = cx.subscribe(
            &query,
            |picker: &mut Picker, _, event: &FieldEvent, cx| match event {
                FieldEvent::Changed => {
                    picker.at = picker.nearest(cx);
                    picker.reveal(cx);
                }
                FieldEvent::Step(by) => picker.step(*by, cx),
                FieldEvent::Enter => {
                    if let Some(id) = picker.rows(cx).get(picker.at).map(|row| row.id) {
                        cx.emit(PickerEvent::Pick(id));
                    }
                }
                FieldEvent::Escape => cx.emit(PickerEvent::Escape),
            },
        );
        Picker {
            store,
            current,
            query,
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

impl Picker {
    pub fn place(&mut self, zoom: f32, width: f32, room: f32, above: bool, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.width = width;
        self.room = room;
        let moved = !self.placed || self.above != above;
        self.above = above;
        if moved {
            self.placed = true;
            self.at = self.start(cx);
            self.reveal(cx);
        }
    }

    fn groups(&self, cx: &App) -> [(Str, Vec<Pickable>); 2] {
        let (marks, all) = self.lists(cx);
        let marks = (Str::BookmarkTitle, marks);
        let all = (Str::ComposerPickAll, all);
        if self.above {
            [all, marks]
        } else {
            [marks, all]
        }
    }

    fn nearest(&self, cx: &App) -> usize {
        if self.above {
            self.rows(cx).len().saturating_sub(1)
        } else {
            0
        }
    }

    fn start(&self, cx: &App) -> usize {
        let groups = self.groups(cx);
        let find = |group: usize| {
            let before: usize = groups[..group].iter().map(|(_, rows)| rows.len()).sum();
            groups[group]
                .1
                .iter()
                .position(|row| row.id == self.current)
                .map(|at| before + at)
        };
        let marks = usize::from(self.above);
        find(marks)
            .or_else(|| find(1 - marks))
            .unwrap_or_else(|| self.nearest(cx))
    }

    fn child_of(&self, at: usize, cx: &App) -> usize {
        let mut before = 0;
        let mut heads = 0;
        for (_, rows) in self.groups(cx) {
            if rows.is_empty() {
                continue;
            }
            heads += 1;
            if at < before + rows.len() {
                break;
            }
            before += rows.len();
        }
        heads + at
    }

    fn reveal(&mut self, cx: &mut Context<Self>) {
        if self.above {
            self.bar.handle().scroll_to_bottom();
        } else {
            self.bar.reset();
        }
        self.bar.handle().scroll_to_item(self.child_of(self.at, cx));
        cx.notify();
    }

    fn lists(&self, cx: &App) -> (Vec<Pickable>, Vec<Pickable>) {
        let Some(snapshot) = self.store.read(cx).snapshot() else {
            return (Vec::new(), Vec::new());
        };
        let text = self.query.read(cx).text().to_owned();
        pickables(snapshot, *cx.global::<Lang>(), &text)
    }

    fn rows(&self, cx: &App) -> Vec<Pickable> {
        self.groups(cx)
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .collect()
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.rows(cx).len() as isize;
        if count == 0 {
            return;
        }
        self.at = (self.at as isize + by).rem_euclid(count) as usize;
        self.bar.handle().scroll_to_item(self.child_of(self.at, cx));
        cx.notify();
    }

    fn row(&self, n: usize, item: &Pickable, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ui::colors(cx);
        let z = self.zoom;
        let id = item.id;
        div()
            .id(("composer-pick-row", n))
            .flex()
            .items_center()
            .gap(px(8.0 * z))
            .min_h(px(40.0 * z))
            .px(px(8.0 * z))
            .py(px(3.0 * z))
            .rounded(px(6.0 * z))
            .cursor_pointer()
            .when(n == self.at, |d| d.bg(c.surface_hover))
            .on_mouse_move(cx.listener(move |picker, _: &MouseMoveEvent, _, cx| {
                if picker.at != n {
                    picker.at = n;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Pick(id))))
            .child(dot(item.dot, &c))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0 * z))
                            .text_color(c.ink_3)
                            .child(item.place.clone()),
                    ),
            )
            .children(
                (item.id == self.current).then(|| div().text_color(c.ink_2).child("\u{2713}")),
            )
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ui::colors(cx);
        let z = self.zoom;
        let groups = self.groups(cx);
        let anchor = self.anchor.clone();
        let head = |text: &'static str| {
            div()
                .px(px(8.0 * z))
                .pt(px(6.0 * z))
                .pb(px(2.0 * z))
                .text_size(px(11.0 * z))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c.ink_3)
                .child(text)
        };
        let mut list = div()
            .id("composer-pick-list")
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(self.bar.handle())
            .px(px(6.0 * z))
            .pb(px(6.0 * z))
            .pt(px(if self.above { 6.0 } else { 0.0 } * z));
        let empty = groups.iter().all(|(_, rows)| rows.is_empty());
        let mut n = 0;
        for (title, rows) in &groups {
            if rows.is_empty() {
                continue;
            }
            list = list.child(head(ui::say(cx, *title)));
            for item in rows {
                list = list.child(self.row(n, item, cx));
                n += 1;
            }
        }
        let find = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0 * z))
            .m(px(6.0 * z))
            .px(px(8.0 * z))
            .h(px(32.0 * z))
            .rounded(px(6.0 * z))
            .border_1()
            .border_color(c.line_strong)
            .bg(c.surface)
            .text_size(px(13.0 * z))
            .child(icon(Lucide::Search, 14.0 * z, c.ink_3))
            .child(self.query.clone());
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
            .child(find);
        if empty {
            body.child(
                div()
                    .px(px(14.0 * z))
                    .pt(px(if self.above { 12.0 } else { 4.0 } * z))
                    .pb(px(if self.above { 4.0 } else { 12.0 } * z))
                    .text_color(c.ink_3)
                    .child(ui::say(cx, Str::ComposerPickNone)),
            )
        } else {
            body.child(self.bar.reserved(list).min_h_0())
        }
    }
}
