use api::settings::{FloatPlace, FloatSize};
use gpui::{Pixels, Point, Size, point, px};

pub const MARGIN: f32 = 12.0;
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
