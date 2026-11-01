use gpui::{
    Context, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollEvent, Window,
};

use super::{TerminalView, metrics};
use crate::vt::view::{Grain, Wheel};

const ARROW_LIMIT: usize = 8;

struct Spot {
    col: usize,
    row: usize,
    right: bool,
    beyond: i32,
}

impl TerminalView {
    fn spot(&self, position: Point<Pixels>, window: &Window) -> Option<Spot> {
        let screen = &self.frame.as_ref()?.screen;
        let cell = metrics(window, self.scale);
        let rel = position - self.area.origin;
        let x = rel.x / cell.cell_width;
        let y = rel.y / cell.cell_height;
        let col = x.max(0.0).floor() as usize;
        let row = y.max(0.0).floor() as usize;
        Some(Spot {
            col: col.min(screen.cols - 1),
            row: row.min(screen.rows - 1),
            right: x.fract().abs() >= 0.5,
            beyond: if y < 0.0 {
                1
            } else if row >= screen.rows {
                -1
            } else {
                0
            },
        })
    }

    pub(super) fn press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus(window, cx);
        cx.stop_propagation();
        let Some(spot) = self.spot(event.position, window) else {
            return;
        };
        let grain = match event.click_count {
            1 => Grain::Cell,
            2 => Grain::Word,
            _ => Grain::Line,
        };
        self.vt.select_start(grain, spot.col, spot.row, spot.right);
        self.picking = true;
        self.changed(cx);
    }

    pub(super) fn drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.picking {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.release(cx);
            return;
        }
        let Some(spot) = self.spot(event.position, window) else {
            return;
        };
        self.vt.scroll_display(spot.beyond);
        self.vt.select_extend(spot.col, spot.row, spot.right);
        self.changed(cx);
    }

    pub(super) fn release(&mut self, cx: &mut Context<Self>) {
        if self.picking {
            self.picking = false;
            self.vt.select_end();
            self.changed(cx);
        }
    }

    pub(super) fn wheel(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.control || event.modifiers.platform {
            return;
        }
        cx.stop_propagation();
        let height = metrics(window, self.scale).cell_height;
        self.wheel_rest += event.delta.pixel_delta(height).y / height;
        let whole = self.wheel_rest.trunc();
        self.wheel_rest -= whole;
        let lines = whole as i32;
        if lines == 0 {
            return;
        }
        match self.vt.wheel() {
            Wheel::History => {
                self.vt.scroll_display(lines);
                self.changed(cx);
            }
            Wheel::Arrows { app_cursor } => {
                let letter = if lines > 0 { 'A' } else { 'B' };
                let lead = if app_cursor { "\x1bO" } else { "\x1b[" };
                let steps = lines.unsigned_abs() as usize;
                let bytes = format!("{lead}{letter}").repeat(steps.min(ARROW_LIMIT));
                self.send(bytes.into_bytes());
            }
            Wheel::Report => {}
        }
    }
}
