use gpui::{App, Keystroke};

use super::layout::Direction;
use super::scene::Scene;

pub fn direction_of(keystroke: &Keystroke, terminal_focused: bool, cx: &App) -> Option<Direction> {
    let modifiers = keystroke.modifiers;
    let chord = modifiers.secondary(cx) && modifiers.shift && !modifiers.alt;
    let plain = !terminal_focused && !modifiers.modified();
    if !chord && !plain {
        return None;
    }
    match keystroke.key.as_str() {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
}

pub fn neighbor(scene: &Scene, from: u32, direction: Direction) -> Option<u32> {
    let origin = scene.cards.iter().find(|c| c.node == from)?.rect.at;
    scene
        .cards
        .iter()
        .filter_map(|c| {
            let dx = c.rect.at.x - origin.x;
            let dy = c.rect.at.y - origin.y;
            let (ahead, aside) = match direction {
                Direction::Left => (-dx, dy),
                Direction::Right => (dx, dy),
                Direction::Up => (-dy, dx),
                Direction::Down => (dy, dx),
            };
            (ahead > 0.0).then(|| (c.node, ahead + 2.0 * aside.abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(node, _)| node)
}
