use std::cell::Cell;
use std::rc::Rc;

use api::settings::{FloatPlace, FloatSize};
use gpui::{
    App, Bounds, Context, Entity, EntityId, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Size, Styled, Window, canvas, point, px,
};

pub const MARGIN: f32 = 12.0;
pub const POP_GAP: f32 = 6.0;
const SLOP: f32 = 4.0;

pub fn zoom(size: FloatSize) -> f32 {
    match size {
        FloatSize::Small => 0.85,
        FloatSize::Normal => 1.0,
        FloatSize::Large => 1.15,
        FloatSize::Huge => 1.3,
    }
}

fn unit(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn room(pane: Size<Pixels>, boxed: Size<Pixels>) -> (f32, f32) {
    let free = |pane: Pixels, boxed: Pixels| (f32::from(pane - boxed) - 2.0 * MARGIN).max(0.0);
    (
        free(pane.width, boxed.width),
        free(pane.height, boxed.height),
    )
}

pub fn place_of(place: FloatPlace, pane: Size<Pixels>, boxed: Size<Pixels>) -> Point<Pixels> {
    let (width, height) = room(pane, boxed);
    point(
        px(MARGIN + unit(place.x) * width),
        px(MARGIN + unit(place.y) * height),
    )
}

pub fn place_from(at: Point<Pixels>, pane: Size<Pixels>, boxed: Size<Pixels>) -> FloatPlace {
    let (width, height) = room(pane, boxed);
    let part = |at: Pixels, room: f32| {
        if room == 0.0 {
            0.0
        } else {
            unit((f32::from(at) - MARGIN) / room)
        }
    };
    FloatPlace {
        x: part(at.x, width),
        y: part(at.y, height),
    }
}

pub fn popup_left(float_left: f32, float_width: f32, popup_width: f32, pane_width: f32) -> f32 {
    let centred = float_left + (float_width - popup_width) / 2.0;
    let upper = (pane_width - MARGIN - popup_width).max(MARGIN);
    centred.clamp(MARGIN, upper) - float_left
}

pub struct Grab {
    from: Point<Pixels>,
    origin: Point<Pixels>,
    moved: bool,
}

impl Grab {
    pub fn new(from: Point<Pixels>, origin: Point<Pixels>) -> Self {
        Self {
            from,
            origin,
            moved: false,
        }
    }

    pub fn moved(&self) -> bool {
        self.moved
    }

    pub fn drag(&mut self, now: Point<Pixels>) -> Option<Point<Pixels>> {
        let delta = now - self.from;
        let far = f32::from(delta.x).hypot(f32::from(delta.y)) > SLOP;
        if !far && !self.moved {
            return None;
        }
        self.moved = true;
        Some(self.origin + delta)
    }
}

pub fn patch(key: &str, place: Option<FloatPlace>) -> serde_json::Value {
    let value = place.map_or(
        serde_json::Value::Null,
        |p| serde_json::json!({ "x": p.x, "y": p.y }),
    );
    serde_json::json!({ key: value })
}

pub fn pop_space(
    top: f32,
    boxed: Size<Pixels>,
    pane: Size<Pixels>,
    cover: Option<Bounds<Pixels>>,
    zoom: f32,
) -> (bool, f32) {
    let height = f32::from(boxed.height);
    let (ceiling, floor) = cover.map_or((0.0, f32::from(pane.height)), |cover| {
        let (cover_top, cover_bottom) = (f32::from(cover.origin.y), f32::from(cover.bottom()));
        (
            if cover_top < top {
                top.min(cover_bottom)
            } else {
                0.0
            },
            if cover_bottom > top + height {
                (top + height).max(cover_top)
            } else {
                f32::from(pane.height)
            },
        )
    });
    let above = top - ceiling - MARGIN;
    let below = floor - (top + height) - MARGIN;
    let use_above = above > below;
    let room = (if use_above { above } else { below }) - POP_GAP * zoom;
    if room <= 0.0 && cover.is_some() {
        return pop_space(top, boxed, pane, None, zoom);
    }
    (use_above, room.max(0.0))
}

pub fn measure(cell: Rc<Cell<Size<Pixels>>>, owner: EntityId) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            if cell.get() != bounds.size {
                cell.set(bounds.size);
                cx.notify(owner);
            }
        },
        |_, _, _, _| (),
    )
    .absolute()
    .size_full()
}

pub fn follow<T: 'static>(
    view: Entity<T>,
    slide: fn(&mut T, &MouseMoveEvent, &mut Context<T>),
    release: fn(&mut T, &mut Context<T>),
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window: &mut Window, _| {
            let moves = view.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, _, _, cx: &mut App| {
                moves.update(cx, |this, cx| slide(this, e, cx));
            });
            let ups = view.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, _, _, cx: &mut App| {
                if e.button == MouseButton::Left {
                    ups.update(cx, release);
                }
            });
        },
    )
    .absolute()
    .size(px(1.0))
}

pub fn stop(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}
