use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Corners, Div, IntoElement, ListState, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathBuilder, Pixels, Rgba, ScrollHandle, Styled, UniformListScrollHandle,
    Window, canvas, div, fill, point, px, rgb,
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
}

#[derive(Clone)]
enum Source {
    Scroll(ScrollHandle),
    Uniform(UniformListScrollHandle),
    List(ListState),
}

impl Source {
    fn max(&self) -> f32 {
        match self {
            Source::Scroll(h) => h.max_offset().y.as_f32().abs(),
            Source::Uniform(h) => h.0.borrow().base_handle.max_offset().y.as_f32().abs(),
            Source::List(l) => l.max_offset_for_scrollbar().y.as_f32().abs(),
        }
    }

    fn offset(&self) -> f32 {
        match self {
            Source::Scroll(h) => -h.offset().y.as_f32(),
            Source::Uniform(h) => -h.0.borrow().base_handle.offset().y.as_f32(),
            Source::List(l) => -l.scroll_px_offset_for_scrollbar().y.as_f32(),
        }
    }

    fn set(&self, y: f32) {
        match self {
            Source::Scroll(h) => h.set_offset(point(h.offset().x, px(-y))),
            Source::Uniform(h) => {
                let base = &h.0.borrow().base_handle;
                base.set_offset(point(base.offset().x, px(-y)));
            }
            Source::List(l) => l.set_offset_from_scrollbar(point(px(0.0), px(-y))),
        }
    }

    fn drag(&self, on: bool) {
        if let Source::List(l) = self {
            if on {
                l.scrollbar_drag_started();
            } else {
                l.scrollbar_drag_ended();
            }
        }
    }
}

#[derive(Clone)]
pub struct Bar {
    source: Source,
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

impl Default for Bar {
    fn default() -> Bar {
        Bar::new()
    }
}

impl Bar {
    fn with(source: Source) -> Bar {
        Bar {
            source,
            corner: 0.0,
            shared: Rc::new(Shared {
                grab: Cell::new(None),
                hover: Cell::new(false),
            }),
        }
    }

    pub fn round(mut self, radius: f32) -> Bar {
        self.corner = radius;
        self
    }

    pub fn new() -> Bar {
        Bar::with(Source::Scroll(ScrollHandle::new()))
    }

    pub fn uniform() -> Bar {
        Bar::with(Source::Uniform(UniformListScrollHandle::new()))
    }

    pub fn uniform_handle(&self) -> Option<UniformListScrollHandle> {
        match &self.source {
            Source::Uniform(h) => Some(h.clone()),
            _ => None,
        }
    }

    pub fn list(state: &ListState) -> Bar {
        Bar::with(Source::List(state.clone()))
    }

    pub fn handle(&self) -> ScrollHandle {
        match &self.source {
            Source::Scroll(h) => h.clone(),
            Source::Uniform(h) => h.0.borrow().base_handle.clone(),
            Source::List(_) => ScrollHandle::new(),
        }
    }

    fn overflows(&self) -> bool {
        self.source.max() > 0.5
    }

    fn metrics(&self, height: f32) -> Option<Metrics> {
        let max = self.source.max();
        if max <= 0.5 {
            return None;
        }
        let view = height;
        let offset = self.source.offset().clamp(0.0, max);
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
        self.source.set(y.clamp(0.0, max));
    }

    pub fn wrap(&self, area: impl IntoElement + Styled, gutter: bool) -> Div {
        let show = gutter || self.overflows();
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
                            |bounds, _, _| bounds,
                            move |bounds, _, window, cx| bar.paint(bounds, show, gutter, window, cx),
                        )
                        .size_full(),
                    ),
            )
    }

    fn paint(&self, bounds: Bounds<Pixels>, shown: bool, gutter: bool, window: &mut Window, cx: &mut App) {
        if !gutter && self.overflows() != shown {
            window.refresh();
        }
        let height = bounds.size.height.as_f32();
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
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            if !phase.bubble() || event.button != MouseButton::Left || !bounds.contains(&event.position)
            {
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
                bar.source.drag(true);
            } else {
                let page = (m.view * 0.875).max(STEP);
                let delta = if y < m.top { -page } else { page };
                bar.scroll_to(m.offset + delta, m.max);
            }
            window.refresh();
        });
        let bar = self.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, _, window, _| {
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
                    bar.source.drag(false);
                }
                window.refresh();
                return;
            }
            let over = bounds.contains(&event.position) && y >= m.top && y <= m.top + m.thumb;
            if over != bar.shared.hover.get() {
                bar.shared.hover.set(over);
                window.refresh();
            }
        });
        let bar = self.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, _, window, _| {
            if bar.shared.grab.take().is_some() {
                bar.source.drag(false);
                window.refresh();
            }
        });
    }
}
