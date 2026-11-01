use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Corners, Div, Hitbox, HitboxBehavior, IntoElement, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Pixels, Rgba,
    ScrollHandle, Styled, UniformListScrollHandle, Window, canvas, div, fill, point, px, rgb,
};

use crate::theme::{Scheme, Theme};

const WIDTH: f32 = 15.0;
const ARROW: f32 = 15.0;
const INSET: f32 = 3.0;
const THUMB: f32 = 9.0;
const MIN_THUMB: f32 = 20.0;
const STEP: f32 = 40.0;
const GLYPH_W: f32 = 9.0;
const GLYPH_H: f32 = 4.5;
const GLYPH_GAP: f32 = 6.5;

struct Shared {
    grab: Cell<Option<f32>>,
    hover: Cell<bool>,
    shown: Cell<bool>,
    flips: Cell<u8>,
    height: Cell<f32>,
}

pub trait Track: Clone + 'static {
    fn max(&self) -> f32;
    fn offset(&self) -> f32;
    fn set(&self, y: f32);
    fn drag(&self, _on: bool) {}
}

impl Track for ScrollHandle {
    fn max(&self) -> f32 {
        self.max_offset().y.as_f32().abs()
    }

    fn offset(&self) -> f32 {
        -ScrollHandle::offset(self).y.as_f32()
    }

    fn set(&self, y: f32) {
        self.set_offset(point(ScrollHandle::offset(self).x, px(-y)));
    }
}

impl Track for UniformListScrollHandle {
    fn max(&self) -> f32 {
        self.0.borrow().base_handle.max()
    }

    fn offset(&self) -> f32 {
        Track::offset(&self.0.borrow().base_handle)
    }

    fn set(&self, y: f32) {
        self.0.borrow().base_handle.set(y);
    }
}

impl Track for ListState {
    fn max(&self) -> f32 {
        self.max_offset_for_scrollbar().y.as_f32().abs()
    }

    fn offset(&self) -> f32 {
        -self.scroll_px_offset_for_scrollbar().y.as_f32()
    }

    fn set(&self, y: f32) {
        self.set_offset_from_scrollbar(point(px(0.0), px(-y)));
    }

    fn drag(&self, on: bool) {
        if on {
            self.scrollbar_drag_started();
        } else {
            self.scrollbar_drag_ended();
        }
    }
}

const FLIPS: u8 = 4;

#[derive(Clone)]
pub struct Bar<T: Track = ScrollHandle> {
    track: T,
    corner: f32,
    shared: Rc<Shared>,
}

struct Palette {
    track: Rgba,
    thumb: Rgba,
    hover: Rgba,
    press: Rgba,
}

fn palette(cx: &App) -> Palette {
    match cx.global::<Theme>().scheme {
        Scheme::Light => Palette {
            track: rgb(0xfcfcfc),
            thumb: rgb(0x8b8b8b),
            hover: rgb(0x7d7d7d),
            press: rgb(0x636363),
        },
        Scheme::Dark => Palette {
            track: rgb(0x2c2c2c),
            thumb: rgb(0x9f9f9f),
            hover: rgb(0xbdbdbd),
            press: rgb(0xd1d1d1),
        },
    }
}

struct Metrics {
    start: f32,
    room: f32,
    thumb: f32,
    top: f32,
    max: f32,
    view: f32,
    offset: f32,
}

impl<T: Track + Default> Default for Bar<T> {
    fn default() -> Bar<T> {
        Bar::over(T::default())
    }
}

impl Bar {
    pub fn new() -> Bar {
        Bar::over(ScrollHandle::new())
    }
}

impl<T: Track> Bar<T> {
    pub fn over(track: T) -> Bar<T> {
        Bar {
            track,
            corner: 0.0,
            shared: Rc::new(Shared {
                grab: Cell::new(None),
                hover: Cell::new(false),
                shown: Cell::new(false),
                flips: Cell::new(0),
                height: Cell::new(0.0),
            }),
        }
    }

    pub fn round(mut self, radius: f32) -> Bar<T> {
        self.corner = radius;
        self
    }

    pub fn handle(&self) -> &T {
        &self.track
    }

    pub fn reset(&self) {
        self.track.set(0.0);
    }

    fn overflows(&self) -> bool {
        self.track.max() > 0.5
    }

    fn metrics(&self, height: f32) -> Option<Metrics> {
        let max = self.track.max();
        if max <= 0.5 {
            return None;
        }
        let view = height;
        let offset = self.track.offset().clamp(0.0, max);
        let start = ARROW + INSET;
        let room = (height - 2.0 * start).max(0.0);
        let thumb = (room * view / (view + max)).clamp(MIN_THUMB.min(room), room);
        let top = start + offset / max * (room - thumb);
        Some(Metrics {
            start,
            room,
            thumb,
            top,
            max,
            view,
            offset,
        })
    }

    fn scroll_to(&self, y: f32, max: f32) {
        self.track.set(y.clamp(0.0, max));
    }

    pub fn reserved(&self, area: impl IntoElement + Styled) -> Div {
        self.column(area, true)
    }

    pub fn on_overflow(&self, area: impl IntoElement + Styled) -> Div {
        self.column(area, false)
    }

    fn column(&self, area: impl IntoElement + Styled, gutter: bool) -> Div {
        let show = gutter || self.shared.shown.get();
        let bar = self.clone();
        div()
            .flex()
            .flex_row()
            .child(area.flex_1().min_w_0())
            .child(
                div()
                    .flex_none()
                    .w(px(if show { WIDTH } else { 0.0 }))
                    .child(
                        canvas(
                            |bounds, window, _| {
                                (bounds, window.insert_hitbox(bounds, HitboxBehavior::Normal))
                            },
                            move |_, (bounds, hitbox), window, cx| {
                                bar.paint(bounds, hitbox, show, gutter, window, cx)
                            },
                        )
                        .size_full(),
                    ),
            )
    }

    fn settle(&self, height: f32, shown: bool, window: &mut Window) {
        if (self.shared.height.get() - height).abs() > 0.5 {
            self.shared.height.set(height);
            self.shared.flips.set(0);
        }
        let wants = self.overflows();
        if wants == shown {
            self.shared.flips.set(0);
        } else if self.shared.flips.get() < FLIPS {
            self.shared.flips.set(self.shared.flips.get() + 1);
            self.shared.shown.set(wants);
            window.refresh();
        }
    }

    fn paint(
        &self,
        bounds: Bounds<Pixels>,
        hitbox: Hitbox,
        shown: bool,
        gutter: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let height = bounds.size.height.as_f32();
        if !gutter {
            self.settle(height, shown, window);
        }
        let Some(m) = self.metrics(height) else {
            return;
        };
        if !shown {
            return;
        }
        let colors = palette(cx);
        let thumb_color = if self.shared.grab.get().is_some() {
            colors.press
        } else if self.shared.hover.get() {
            colors.hover
        } else {
            colors.thumb
        };
        window.paint_quad(fill(bounds, colors.track).corner_radii(Corners {
            top_left: px(0.0),
            top_right: px(self.corner),
            bottom_right: px(self.corner),
            bottom_left: px(0.0),
        }));
        let thumb = Bounds::new(
            point(
                bounds.origin.x + px((WIDTH - THUMB) / 2.0),
                bounds.origin.y + px(m.top),
            ),
            gpui::size(px(THUMB), px(m.thumb)),
        );
        window.paint_quad(fill(thumb, thumb_color).corner_radii(px(THUMB / 2.0)));
        let glyph = |up: bool| {
            let cx0 = bounds.origin.x + px(WIDTH / 2.0);
            let (base, tip) = if up {
                let y = bounds.origin.y + px(GLYPH_GAP);
                (y + px(GLYPH_H), y)
            } else {
                let y = bounds.origin.y + bounds.size.height - px(GLYPH_GAP);
                (y - px(GLYPH_H), y)
            };
            let mut path = PathBuilder::fill();
            path.move_to(point(cx0 - px(GLYPH_W / 2.0), base));
            path.line_to(point(cx0 + px(GLYPH_W / 2.0), base));
            path.line_to(point(cx0, tip));
            path.close();
            path.build()
        };
        for up in [true, false] {
            if let Ok(path) = glyph(up) {
                window.paint_path(path, colors.thumb);
            }
        }

        let bar = self.clone();
        let down = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            if !phase.bubble() || event.button != MouseButton::Left || !down.is_hovered(window) {
                return;
            }
            let Some(m) = bar.metrics(height) else {
                return;
            };
            let y = (event.position.y - bounds.origin.y).as_f32();
            if y < ARROW {
                bar.scroll_to(m.offset - STEP, m.max);
            } else if y > height - ARROW {
                bar.scroll_to(m.offset + STEP, m.max);
            } else if y >= m.top && y <= m.top + m.thumb {
                bar.shared.grab.set(Some(y - m.top));
                bar.track.drag(true);
            } else {
                let page = (m.view * 0.875).max(STEP);
                let delta = if y < m.top { -page } else { page };
                bar.scroll_to(m.offset + delta, m.max);
            }
            window.refresh();
        });
        let bar = self.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
            if !phase.bubble() {
                return;
            }
            let Some(m) = bar.metrics(height) else {
                return;
            };
            let y = (event.position.y - bounds.origin.y).as_f32();
            if let Some(grab) = bar.shared.grab.get() {
                if event.pressed_button == Some(MouseButton::Left) {
                    let span = (m.room - m.thumb).max(1.0);
                    let top = (y - grab - m.start).clamp(0.0, span);
                    bar.scroll_to(top / span * m.max, m.max);
                } else {
                    bar.shared.grab.set(None);
                    bar.track.drag(false);
                }
                window.refresh();
                return;
            }
            let over = hitbox.is_hovered(window) && y >= m.top && y <= m.top + m.thumb;
            if over != bar.shared.hover.get() {
                bar.shared.hover.set(over);
                window.refresh();
            }
        });
        let bar = self.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _| {
            if !phase.bubble() {
                return;
            }
            if bar.shared.grab.take().is_some() {
                bar.track.drag(false);
                window.refresh();
            }
        });
    }
}
