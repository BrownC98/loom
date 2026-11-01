use std::cell::Cell;
use std::rc::Rc;

use api::graph::waits::{ScreenWait, WaitTone};
use api::ids::WaitId;
use api::settings::{FloatPlace, FloatSize, QuestionPill};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, BoxShadow, Context, Div, Entity, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    ScrollEvent, Size, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div,
    point, px,
};

use super::place::{self, Grab, MARGIN};
use crate::store::Store;
use crate::strings::Str;
use crate::views::dock::answers::{self, Answers};
use crate::views::dock::questions::{Group, groups_of, kind_label};
use crate::views::dock::ui::{self, GAP};
use crate::views::tooltip::tip;

const HOME: FloatPlace = FloatPlace { x: 0.5, y: 0.0 };
const WIDTH: f32 = 520.0;
const HEAD: f32 = 34.0;
const POP_GAP: f32 = 6.0;
const SHADOW_BLUR: f32 = 14.0;
const SHADOW_DROP: f32 = 4.0;

pub struct Pill {
    store: Entity<Store>,
    pane: Rc<Cell<Size<Pixels>>>,
    boxed: Rc<Cell<Size<Pixels>>>,
    open: bool,
    grab: Option<Grab>,
    dragged: bool,
    at: Option<Point<Pixels>>,
    kept: Option<Option<FloatPlace>>,
    seen_place: Option<FloatPlace>,
    shut: Option<WaitId>,
    seen_shut: Option<WaitId>,
    answers: Entity<Answers>,
    subscriptions: Vec<Subscription>,
}

pub fn pill(store: Entity<Store>, _: &mut Window, cx: &mut App) -> Entity<Pill> {
    cx.new(|cx| {
        let settings = store.read(cx).settings();
        let seen_place = settings.pill_place;
        let seen_shut = settings.closed_pill;
        let mut pill = Pill {
            store: store.clone(),
            pane: Rc::new(Cell::new(Size::default())),
            boxed: Rc::new(Cell::new(Size::default())),
            open: false,
            grab: None,
            dragged: false,
            at: None,
            kept: None,
            seen_place,
            shut: None,
            seen_shut,
            answers: cx.new(|cx| Answers::new(store.clone(), cx)),
            subscriptions: Vec::new(),
        };
        pill.subscriptions
            .push(cx.observe(&store, |pill: &mut Pill, _, cx| pill.sync(cx)));
        pill.subscriptions
            .push(cx.observe(&pill.answers, |_, _, cx| cx.notify()));
        pill.sync(cx);
        pill
    })
}

struct Look {
    view: QuestionPill,
    size: FloatSize,
    saved: Option<FloatPlace>,
}

fn is_shut(closed: Option<WaitId>, groups: &[Group]) -> bool {
    closed.is_some_and(|c| {
        groups
            .iter()
            .flat_map(|g| &g.waits)
            .all(|w| w.wait.id.0 <= c.0)
    })
}

impl Pill {
    fn sync(&mut self, cx: &mut Context<Self>) {
        let settings = self.store.read(cx).settings();
        let (place, shut) = (settings.pill_place, settings.closed_pill);
        if place != self.seen_place {
            self.seen_place = place;
            self.kept = None;
        }
        if shut != self.seen_shut {
            self.seen_shut = shut;
            self.shut = None;
        }
        cx.notify();
    }

    fn groups(&self, cx: &Context<Self>) -> Vec<Group> {
        self.store
            .read(cx)
            .snapshot()
            .map(groups_of)
            .unwrap_or_default()
    }

    fn look(&self, cx: &Context<Self>) -> Look {
        let settings = self.store.read(cx).settings();
        Look {
            view: settings.question_pill,
            size: settings.pill_size,
            saved: self.kept.unwrap_or(settings.pill_place),
        }
    }

    fn put(&mut self, patch: serde_json::Value, cx: &mut Context<Self>) {
        self.store
            .update(cx, |store, cx| store.save_settings(patch, cx));
    }

    fn grab(&mut self, event: &MouseDownEvent, origin: Point<Pixels>, cx: &mut Context<Self>) {
        self.dragged = false;
        self.grab = Some(Grab::new(event.position, origin));
        cx.notify();
    }

    fn slide(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(grab) = &mut self.grab else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.grab = None;
            return;
        }
        let Some(origin) = grab.drag(event.position) else {
            return;
        };
        let (pane, boxed) = (self.pane.get(), self.boxed.get());
        self.dragged = true;
        self.at = Some(place::place_of(
            place::place_from(origin, pane, boxed),
            pane,
            boxed,
        ));
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        let Some(grab) = self.grab.take() else { return };
        if !grab.moved() {
            return;
        }
        if let Some(at) = self.at.take() {
            let dropped = place::place_from(at, self.pane.get(), self.boxed.get());
            self.kept = Some(Some(dropped));
            self.put(place::patch("pillPlace", Some(dropped)), cx);
        }
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.kept = Some(None);
        self.put(place::patch("pillPlace", None), cx);
        cx.notify();
    }

    fn close(&mut self, newest: WaitId, cx: &mut Context<Self>) {
        self.shut = Some(newest);
        self.open = false;
        self.put(serde_json::json!({ "closedPill": newest.0 }), cx);
        cx.notify();
    }

    fn jump(&mut self, node: api::ids::NodeId, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.select(node.0, cx));
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.dragged {
            return;
        }
        self.open = !self.open;
        cx.notify();
    }
}

fn follow(pill: Entity<Pill>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let moves = pill.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, _, _, cx| {
                moves.update(cx, |pill, cx| pill.slide(e, cx));
            });
            let ups = pill.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, _, _, cx| {
                if e.button == MouseButton::Left {
                    ups.update(cx, |pill, cx| pill.release(cx));
                }
            });
        },
    )
    .absolute()
    .size(px(1.0))
}

fn stop(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

impl Render for Pill {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = cx.entity_id();
        let (pane_cell, box_cell) = (self.pane.clone(), self.boxed.clone());
        let measure = canvas(
            move |bounds, _, cx| {
                if pane_cell.get() != bounds.size {
                    pane_cell.set(bounds.size);
                    cx.notify(id);
                }
            },
            |_, _, _, _| (),
        )
        .absolute()
        .size_full();
        let root = div().absolute().top_0().left_0().size_full().child(measure);
        let groups = self.groups(cx);
        let look = self.look(cx);
        let closed = [self.shut, self.store.read(cx).settings().closed_pill]
            .into_iter()
            .flatten()
            .max_by_key(|w| w.0);
        let count: usize = groups.iter().map(|g| g.waits.len()).sum();
        if look.view == QuestionPill::Off || count == 0 || is_shut(closed, &groups) {
            self.open = false;
            return root;
        }
        let z = place::zoom(look.size);
        let (pane, boxed) = (self.pane.get(), self.boxed.get());
        let measured = boxed.width > px(0.0) && pane.width > px(0.0);
        let at = self
            .at
            .unwrap_or_else(|| place::place_of(look.saved.unwrap_or(HOME), pane, boxed));
        let line = look.view == QuestionPill::Line;
        let oldest = &groups[0];
        let first = &oldest.waits[0];
        let newest = groups
            .iter()
            .flat_map(|g| &g.waits)
            .map(|w| w.wait.id)
            .max_by_key(|w| w.0)
            .unwrap_or(first.wait.id);
        let limit = (f32::from(pane.width) - 2.0 * MARGIN).max(0.0);
        let width = (WIDTH * z).min(limit);
        let face = self.face(&groups, first, &oldest.name, line, z, at, newest, cx);
        let face = div()
            .relative()
            .occlude()
            .when(line, |d| d.max_w(px(width)))
            .when(!line, |d| d.w(px(width)))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        if box_cell.get() != bounds.size {
                            box_cell.set(bounds.size);
                            cx.notify(id);
                        }
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full(),
            )
            .child(face);
        let mut group = div().absolute().left(at.x).top(at.y);
        if !measured {
            group = group.invisible();
        }
        group = group.child(face);
        if self.open {
            group = group.child(self.pop(&groups, count, at, boxed, pane, width, z, cx));
        }
        let follow = self.grab.is_some().then(|| follow(cx.entity()));
        root.children(follow).child(group)
    }
}

impl Pill {
    #[allow(clippy::too_many_arguments)]
    fn face(
        &self,
        groups: &[Group],
        first: &ScreenWait,
        name: &str,
        line: bool,
        z: f32,
        at: Point<Pixels>,
        newest: WaitId,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = ui::colors(cx);
        let tone = match first.tone {
            WaitTone::Ask => c.warn,
            WaitTone::Done => c.ok,
        };
        let waits: Vec<&ScreenWait> = groups.iter().flat_map(|g| &g.waits).collect();
        let asks = waits
            .iter()
            .filter(|w| matches!(w.tone, WaitTone::Ask))
            .count();
        let dones = waits.len() - asks;
        let badge = |mark: &'static str, n: usize, color: gpui::Rgba| {
            div()
                .flex_none()
                .min_w(px(18.0 * z))
                .h(px(18.0 * z))
                .px(px(5.0 * z))
                .rounded(px(9.0 * z))
                .bg(color)
                .text_color(c.warn_ink)
                .text_size(px(11.0 * z))
                .flex()
                .items_center()
                .justify_center()
                .child(format!("{mark}{n}"))
        };
        let tool = |id: &'static str, glyph: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .size(px(24.0 * z))
                .rounded(px(5.0 * z))
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.ink_3)
                .cursor_pointer()
                .hover(move |s| s.bg(c.surface_hover).text_color(c.ink))
                .on_mouse_down(MouseButton::Left, stop)
                .tooltip(tip(label))
                .child(glyph)
        };
        let head_text = if line && !first.summary.is_empty() {
            format!("{name}: {}", first.summary)
        } else {
            name.to_owned()
        };
        let label = if line {
            div().truncate().child(head_text)
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(4.0 * z))
                .child(div().font_weight(gpui::FontWeight::BOLD).child(head_text))
                .child(if self.open { "\u{25b4}" } else { "\u{25be}" })
        };
        let go = div()
            .id("pill-go")
            .flex_1()
            .min_w_0()
            .px(px(4.0 * z))
            .py(px(2.0 * z))
            .rounded(px(5.0 * z))
            .text_size(px(12.0 * z))
            .text_color(c.ink)
            .cursor_pointer()
            .when(self.open, |d| d.bg(c.surface_hover))
            .hover(move |s| s.bg(c.surface_hover))
            .tooltip(tip(ui::say_with(
                cx,
                Str::InboxListLabel,
                &[("count", &waits.len().to_string())],
            )))
            .on_click(cx.listener(|pill, _, _, cx| pill.toggle(cx)))
            .child(label);
        let head = div()
            .id("pill-head")
            .flex()
            .items_center()
            .gap(px(8.0 * z))
            .min_w_0()
            .h(px(HEAD * z))
            .px(px(6.0 * z))
            .text_size(px(13.0 * z))
            .cursor_grab()
            .when(!line, |d| d.border_b_1().border_color(c.line))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |pill, e: &MouseDownEvent, _, cx| pill.grab(e, at, cx)),
            )
            .child(
                tool(
                    "pill-close",
                    "\u{00d7}",
                    ui::say(cx, Str::InboxClosePill).to_owned(),
                )
                .on_click(cx.listener(move |pill, _, _, cx| pill.close(newest, cx))),
            )
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .flex_none()
                    .child(ui::say(cx, kind_label(first.wait.kind))),
            )
            .children((asks > 0).then(|| badge("?", asks, c.warn)))
            .children((dones > 0).then(|| badge("\u{2713}", dones, c.ok)))
            .children(
                (first.wait.kind == api::graph::waits::WaitKind::Question).then(|| {
                    div()
                        .flex_none()
                        .px(px(6.0 * z))
                        .h(px(18.0 * z))
                        .rounded(px(4.0 * z))
                        .border_1()
                        .border_color(tone)
                        .text_size(px(11.0 * z))
                        .flex()
                        .items_center()
                        .child(first.wait.id.to_string())
                }),
            )
            .child(go)
            .children(self.reset_shown(cx).then(|| {
                tool(
                    "pill-reset",
                    "\u{21ba}",
                    ui::say(cx, Str::InboxResetPlace).to_owned(),
                )
                .on_click(cx.listener(|pill, _, _, cx| pill.reset(cx)))
            }));
        let body = (!line).then(|| {
            div()
                .px(px(12.0 * z))
                .pt(px(10.0 * z))
                .pb(px(12.0 * z))
                .child(answers::entry(
                    &self.answers,
                    oldest_node(groups),
                    first,
                    z,
                    cx,
                ))
        });
        div()
            .flex()
            .flex_col()
            .rounded(px(if line { 17.0 } else { 14.0 } * z))
            .border_1()
            .border_color(tone)
            .bg(c.panel)
            .text_color(c.ink)
            .shadow(shadow(c.shadow))
            .on_mouse_down(MouseButton::Left, stop)
            .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
            .child(head)
            .children(body)
    }

    fn reset_shown(&self, cx: &Context<Self>) -> bool {
        self.kept
            .unwrap_or(self.store.read(cx).settings().pill_place)
            .is_some()
    }

    #[allow(clippy::too_many_arguments)]
    fn pop(
        &self,
        groups: &[Group],
        count: usize,
        at: Point<Pixels>,
        boxed: Size<Pixels>,
        pane: Size<Pixels>,
        width: f32,
        z: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = ui::colors(cx);
        let (above, room) = pop_space(f32::from(at.y), boxed, pane, z);
        let left = place::popup_left(
            f32::from(at.x),
            f32::from(boxed.width),
            width,
            f32::from(pane.width),
        );
        let title = ui::say_with(cx, Str::InboxTitle, &[("count", &count.to_string())]);
        let list = div()
            .flex()
            .flex_col()
            .children(groups.iter().map(|g| self.group(g, z, cx)));
        let pop = div()
            .id("pill-pop")
            .absolute()
            .left(px(left))
            .w(px(width))
            .max_h(px(room))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .gap(px(GAP * z))
            .pt(px(10.0 * z))
            .px(px(6.0 * z))
            .pb(px(6.0 * z))
            .rounded(px(10.0 * z))
            .border_1()
            .border_color(c.line_strong)
            .bg(c.surface)
            .text_color(c.ink)
            .text_size(px(13.0 * z))
            .shadow(shadow(c.shadow))
            .occlude()
            .on_mouse_down(MouseButton::Left, stop)
            .on_scroll(|_: &ScrollEvent, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(8.0 * z))
                    .text_size(px(12.0 * z))
                    .text_color(c.ink_3)
                    .child(title),
            )
            .child(list);
        let gap = px((POP_GAP) * z);
        if above {
            pop.bottom(boxed.height + gap)
        } else {
            pop.top(boxed.height + gap)
        }
    }

    fn group(&self, group: &Group, z: f32, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let node = group.node;
        div()
            .flex()
            .flex_col()
            .gap(px(2.0 * z))
            .py(px(4.0 * z))
            .border_t_1()
            .border_color(c.line)
            .child(
                ui::link(("pill-go", node.0 as usize), group.name.clone(), cx)
                    .px(px(8.0 * z))
                    .h(px(28.0 * z))
                    .flex()
                    .items_center()
                    .on_click(cx.listener(move |pill, _, _, cx| pill.jump(node, cx))),
            )
            .children(
                group
                    .waits
                    .iter()
                    .map(|w| answers::entry(&self.answers, node, w, z, cx)),
            )
    }
}

fn oldest_node(groups: &[Group]) -> api::ids::NodeId {
    groups[0].node
}

fn pop_space(top: f32, boxed: Size<Pixels>, pane: Size<Pixels>, z: f32) -> (bool, f32) {
    let above = top - MARGIN;
    let below = f32::from(pane.height) - (top + f32::from(boxed.height)) - MARGIN;
    let use_above = above > below;
    let room = (if use_above { above } else { below } - POP_GAP * z).max(0.0);
    (use_above, room)
}

fn shadow(color: gpui::Rgba) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.into(),
        offset: point(px(0.0), px(SHADOW_DROP)),
        blur_radius: px(SHADOW_BLUR),
        spread_radius: px(0.0),
        inset: false,
    }]
}
