use gpui::{
    Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollEvent, Window,
};

use super::{TerminalView, metrics};
use crate::vt::view::Grain;
use crate::vt::{self, Mouse, MouseAction};

struct Spot {
    col: usize,
    row: usize,
    right: bool,
    beyond: i32,
}

fn reported(button: MouseButton) -> Option<vt::MouseButton> {
    match button {
        MouseButton::Left => Some(vt::MouseButton::Left),
        MouseButton::Middle => Some(vt::MouseButton::Middle),
        MouseButton::Right => Some(vt::MouseButton::Right),
        _ => None,
    }
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

    fn report(&mut self, action: MouseAction, spot: &Spot, keys: &Modifiers) -> bool {
        let mouse = Mouse {
            action,
            col: spot.col,
            row: spot.row,
            ctrl: keys.control,
            alt: keys.alt,
            shift: keys.shift,
        };
        let bytes = self.vt.encode_mouse(&mouse);
        let sent = bytes.is_some();
        if let Some(bytes) = bytes {
            self.send(bytes);
        }
        sent
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
        if self.vt.mouse_reporting() && !event.modifiers.shift {
            if let Some(button) = reported(event.button) {
                self.reporting = Some(event.button);
                self.last_cell = Some((spot.col, spot.row));
                self.report(MouseAction::Press(button), &spot, &event.modifiers);
            }
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
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
        let Some(spot) = self.spot(event.position, window) else {
            return;
        };
        let cell = Some((spot.col, spot.row));
        if let Some(held) = self.reporting {
            if event.pressed_button != Some(held) {
                self.reporting = None;
            } else if self.last_cell != cell
                && let Some(button) = reported(held)
            {
                self.last_cell = cell;
                self.report(MouseAction::Drag(button), &spot, &event.modifiers);
            }
            return;
        }
        if self.picking {
            if event.pressed_button != Some(MouseButton::Left) {
                self.finish_picking(cx);
                return;
            }
            self.vt.scroll_display(spot.beyond);
            self.vt.select_extend(spot.col, spot.row, spot.right);
            self.changed(cx);
        } else if event.pressed_button.is_none()
            && self.area.contains(&event.position)
            && self.last_cell != cell
            && self.vt.mouse_reporting()
            && !event.modifiers.shift
        {
            self.last_cell = cell;
            self.report(MouseAction::Move, &spot, &event.modifiers);
        }
    }

    pub(super) fn release(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reporting == Some(event.button) {
            self.reporting = None;
            if let (Some(button), Some(spot)) =
                (reported(event.button), self.spot(event.position, window))
            {
                self.report(MouseAction::Release(button), &spot, &event.modifiers);
            }
        } else if event.button == MouseButton::Left {
            self.finish_picking(cx);
        }
    }

    fn finish_picking(&mut self, cx: &mut Context<Self>) {
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
        if !event.modifiers.shift
            && let Some(spot) = self.spot(event.position, window)
        {
            let action = MouseAction::Wheel {
                up: lines > 0,
                lines: lines.unsigned_abs() as usize,
            };
            if self.report(action, &spot, &event.modifiers) {
                return;
            }
        }
        self.vt.scroll_display(lines);
        self.changed(cx);
    }
}
