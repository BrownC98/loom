use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, BoxShadow, Context, Div, FocusHandle, Image, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement, Point, Render, ScrollEvent, Size, StatefulInteractiveElement, Styled,
    StyledImage, Window, anchored, deferred, div, img, point, px,
};

use super::dock::ui;
use super::tooltip::tip;
use crate::strings::Str;
use crate::theme::Colors;

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 6.0;
const TAP_SLOP: f32 = 8.0;
const FIT_SHARE: f32 = 0.92;
const BUTTON_STEP: f32 = 1.25;
const WHEEL_RATE: f32 = 0.002;
const LINE_PIXELS: f32 = 16.0;
const BAR_BOTTOM: f32 = 18.0;
const BAR_BUTTON: f32 = 32.0;
const BAR_READOUT: f32 = 52.0;

#[derive(Clone, Copy)]
struct View {
    x: f32,
    y: f32,
    zoom: f32,
}

const FIT: View = View {
    x: 0.0,
    y: 0.0,
    zoom: 1.0,
};

#[derive(Clone, Copy)]
struct Fit {
    width: f32,
    height: f32,
    scale: f32,
}

struct Drag {
    from: Point<gpui::Pixels>,
    view: View,
}

pub struct Viewer {
    shown: Option<Arc<Image>>,
    view: View,
    fit: Option<Fit>,
    room: Size<f32>,
    drag: Option<Drag>,
    moved: bool,
    focus: FocusHandle,
    before: Option<FocusHandle>,
}

fn place_axis(pos: f32, size: f32, room: f32) -> f32 {
    if size <= room {
        (room - size) / 2.0
    } else {
        pos.max(room - size).min(0.0)
    }
}

fn contain(view: View, fit: Fit, room: Size<f32>) -> View {
    View {
        zoom: view.zoom,
        x: place_axis(view.x, fit.width * view.zoom, room.width),
        y: place_axis(view.y, fit.height * view.zoom, room.height),
    }
}

fn zoom_about(view: View, zoom: f32, at: (f32, f32)) -> View {
    let next = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    View {
        zoom: next,
        x: at.0 - (at.0 - view.x) * next / view.zoom,
        y: at.1 - (at.1 - view.y) * next / view.zoom,
    }
}

impl Viewer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            shown: None,
            view: FIT,
            fit: None,
            room: Size::default(),
            drag: None,
            moved: false,
            focus: cx.focus_handle(),
            before: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.shown.is_some()
    }

    pub fn open(&mut self, image: Arc<Image>, window: &mut Window, cx: &mut Context<Self>) {
        self.before = window.focused(cx);
        self.shown = Some(image);
        self.view = FIT;
        self.fit = None;
        self.drag = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = None;
        self.drag = None;
        if let Some(before) = self.before.take() {
            window.focus(&before, cx);
        }
        cx.notify();
    }

    fn set(&mut self, view: View, cx: &mut Context<Self>) {
        if let Some(fit) = self.fit {
            self.view = contain(view, fit, self.room);
            cx.notify();
        }
    }

    fn zoom_at(&mut self, zoom: f32, at: (f32, f32), cx: &mut Context<Self>) {
        self.set(zoom_about(self.view, zoom, at), cx);
    }

    fn step(&mut self, factor: f32, cx: &mut Context<Self>) {
        let Some(fit) = self.fit else { return };
        let actual = 1.0 / fit.scale;
        let now = self.view.zoom;
        let next = now * factor;
        let target = if (now < actual && next > actual) || (now > actual && next < actual) {
            actual
        } else {
            next
        };
        self.zoom_at(target, (self.room.width / 2.0, self.room.height / 2.0), cx);
    }

    fn wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let delta = f32::from(event.delta.pixel_delta(px(LINE_PIXELS)).y);
        let at = (f32::from(event.position.x), f32::from(event.position.y));
        self.zoom_at(self.view.zoom * (delta * WHEEL_RATE).exp(), at, cx);
        cx.stop_propagation();
    }

    fn grab(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.moved = false;
        self.drag = Some(Drag {
            from: event.position,
            view: self.view,
        });
        cx.stop_propagation();
    }

    fn slide(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let dx = f32::from(event.position.x - drag.from.x);
        let dy = f32::from(event.position.y - drag.from.y);
        if dx.hypot(dy) > TAP_SLOP {
            self.moved = true;
        }
        if drag.view.zoom > 1.0 {
            let moved = View {
                x: drag.view.x + dx,
                y: drag.view.y + dy,
                zoom: drag.view.zoom,
            };
            self.set(moved, cx);
        }
    }

    fn release(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_none() || self.moved {
            return;
        }
        let Some(fit) = self.fit else { return };
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        let on_image = x >= self.view.x
            && x <= self.view.x + fit.width * self.view.zoom
            && y >= self.view.y
            && y <= self.view.y + fit.height * self.view.zoom;
        if on_image && self.view.zoom > 1.0 {
            self.set(FIT, cx);
        } else {
            self.close(window, cx);
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.close(window, cx);
            cx.stop_propagation();
        }
    }

    fn measure(&mut self, window: &mut Window, cx: &mut App) {
        let size = window.viewport_size();
        self.room = Size {
            width: f32::from(size.width),
            height: f32::from(size.height),
        };
        let Some(shown) = &self.shown else { return };
        let Some(data) = shown.clone().use_render_image(window, cx) else {
            return;
        };
        let natural = data.size(0);
        let (width, height) = (natural.width.0 as f32, natural.height.0 as f32);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let scale = 1f32
            .min(self.room.width * FIT_SHARE / width)
            .min(self.room.height * FIT_SHARE / height);
        let fit = Fit {
            width: width * scale,
            height: height * scale,
            scale,
        };
        self.fit = Some(fit);
        self.view = contain(self.view, fit, self.room);
    }
}

fn bar_button(id: &'static str, label: String, enabled: bool, c: Colors) -> gpui::Stateful<Div> {
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .min_w(px(BAR_BUTTON))
        .h(px(BAR_BUTTON))
        .px(px(8.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .child(label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    if enabled {
        button
            .cursor_pointer()
            .hover(move |style| style.bg(c.surface_hover))
    } else {
        button.opacity(0.4)
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(image) = self.shown.clone() else {
            return div().into_any_element();
        };
        self.measure(window, cx);
        let c = ui::colors(cx);
        let owner = cx.entity();
        let backdrop = div()
            .key_context("ImageViewer")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .absolute()
            .top_0()
            .left_0()
            .w(px(self.room.width))
            .h(px(self.room.height))
            .occlude()
            .overflow_hidden()
            .bg(c.scrim)
            .text_color(c.ink)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|viewer, e: &MouseDownEvent, _, cx| viewer.grab(e, cx)),
            )
            .on_mouse_move(cx.listener(|viewer, e: &MouseMoveEvent, _, cx| viewer.slide(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|viewer, e: &MouseUpEvent, window, cx| viewer.release(e, window, cx)),
            )
            .on_scroll(cx.listener(|viewer, e: &ScrollEvent, _, cx| viewer.wheel(e, cx)));
        let Some(fit) = self.fit else {
            return deferred(anchored().position(point(px(0.0), px(0.0))).child(backdrop))
                .with_priority(2)
                .into_any_element();
        };
        let view = self.view;
        let zoomed = view.zoom > 1.0;
        let picture = div()
            .absolute()
            .left(px(view.x))
            .top(px(view.y))
            .w(px(fit.width * view.zoom))
            .h(px(fit.height * view.zoom))
            .rounded(px(if zoomed { 0.0 } else { 8.0 }))
            .overflow_hidden()
            .bg(c.panel)
            .when(zoomed, |d| d.cursor_grab())
            .when(!zoomed, |d| d.cursor_pointer())
            .child(
                img(image)
                    .size_full()
                    .object_fit(ObjectFit::Fill)
                    .into_element(),
            );
        let percent = (fit.scale * view.zoom * 100.0).round() as i32;
        let bar = div()
            .absolute()
            .bottom(px(BAR_BOTTOM))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .p(px(4.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(c.line_strong)
                    .bg(c.panel)
                    .shadow(vec![BoxShadow {
                        color: c.shadow.into(),
                        offset: point(px(0.0), px(4.0)),
                        blur_radius: px(14.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .child(
                        bar_button("viewer-out", "\u{2212}".to_owned(), view.zoom > MIN_ZOOM, c)
                            .tooltip(tip(ui::say(cx, Str::InboxZoomOut).to_owned()))
                            .on_click({
                                let owner = owner.clone();
                                move |_, _, cx| {
                                    owner
                                        .update(cx, |viewer, cx| viewer.step(1.0 / BUTTON_STEP, cx))
                                }
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(BAR_READOUT))
                            .text_center()
                            .text_size(px(12.0))
                            .text_color(c.ink_2)
                            .child(format!("{percent}%")),
                    )
                    .child(
                        bar_button("viewer-in", "+".to_owned(), view.zoom < MAX_ZOOM, c)
                            .tooltip(tip(ui::say(cx, Str::InboxZoomIn).to_owned()))
                            .on_click({
                                let owner = owner.clone();
                                move |_, _, cx| {
                                    owner.update(cx, |viewer, cx| viewer.step(BUTTON_STEP, cx))
                                }
                            }),
                    )
                    .child(div().w(px(1.0)).h(px(20.0)).mx(px(4.0)).bg(c.line))
                    .child(
                        bar_button(
                            "viewer-fit",
                            ui::say(cx, Str::InboxFitImage).to_owned(),
                            true,
                            c,
                        )
                        .on_click(move |_, _, cx| {
                            owner.update(cx, |viewer, cx| viewer.set(FIT, cx))
                        }),
                    ),
            );
        deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .child(backdrop.child(picture).child(bar)),
        )
        .with_priority(2)
        .into_any_element()
    }
}
