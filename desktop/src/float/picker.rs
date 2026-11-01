use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render,
    ScrollEvent, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::place;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::views::dock::ui;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};
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
    scroll: ScrollHandle,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    zoom: f32,
    width: f32,
    room: f32,
    above: bool,
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
                    picker.at = 0;
                    cx.notify();
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
        let mut picker = Picker {
            store,
            current,
            query,
            at: 0,
            scroll: ScrollHandle::new(),
            anchor,
            zoom: 1.0,
            width: 0.0,
            room: 0.0,
            above: false,
            _subscription: subscription,
        };
        picker.at = picker
            .rows(cx)
            .iter()
            .position(|row| row.id == current)
            .unwrap_or(0);
        picker
    })
}

impl Picker {
    pub fn place(&mut self, zoom: f32, width: f32, room: f32, above: bool) {
        self.zoom = zoom;
        self.width = width;
        self.room = room;
        self.above = above;
    }

    fn lists(&self, cx: &App) -> (Vec<Pickable>, Vec<Pickable>) {
        let Some(snapshot) = self.store.read(cx).snapshot() else {
            return (Vec::new(), Vec::new());
        };
        let text = self.query.read(cx).text().to_owned();
        pickables(snapshot, *cx.global::<Lang>(), &text)
    }

    fn rows(&self, cx: &App) -> Vec<Pickable> {
        let (marks, all) = self.lists(cx);
        marks.into_iter().chain(all).collect()
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.rows(cx).len() as isize;
        if count == 0 {
            return;
        }
        self.at = (self.at as isize + by).rem_euclid(count) as usize;
        let (marks, all) = self.lists(cx);
        let child = if self.at < marks.len() {
            usize::from(!marks.is_empty()) + self.at
        } else {
            let heads = usize::from(!marks.is_empty()) + usize::from(!all.is_empty());
            heads + self.at
        };
        self.scroll.scroll_to_item(child);
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
        let (marks, all) = self.lists(cx);
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
            .track_scroll(&self.scroll)
            .px(px(6.0 * z))
            .pb(px(6.0 * z))
            .pt(px(if self.above { 6.0 } else { 0.0 } * z));
        let empty = marks.is_empty() && all.is_empty();
        if !marks.is_empty() {
            list = list.child(head(ui::say(cx, Str::BookmarkTitle)));
        }
        let split = marks.len();
        for (n, item) in marks.iter().enumerate() {
            list = list.child(self.row(n, item, cx));
        }
        if !all.is_empty() {
            list = list.child(head(ui::say(cx, Str::ComposerPickAll)));
        }
        for (n, item) in all.iter().enumerate() {
            list = list.child(self.row(split + n, item, cx));
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
            body.child(list)
        }
    }
}
