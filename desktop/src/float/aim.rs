use api::graph::waits::{ScreenWait, WaitTone};
use api::ids::WaitId;
use gpui::Rgba;

use crate::theme::Colors;
use crate::views::dock::entry;
use crate::views::icons::Lucide;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aim {
    Alert(WaitId),
    Prompt,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Answer(WaitId),
    Continue(WaitId),
    Prompt,
}

impl Kind {
    pub fn glyph(self) -> Lucide {
        match self {
            Kind::Answer(_) => Lucide::CircleQuestionMark,
            Kind::Continue(_) => Lucide::CircleCheck,
            Kind::Prompt => Lucide::SquareTerminal,
        }
    }

    pub fn paint(self, c: &Colors) -> (Rgba, Rgba) {
        match self {
            Kind::Answer(_) => (
                entry::tone_of(WaitTone::Ask, c),
                entry::tone_ink(WaitTone::Ask, c),
            ),
            Kind::Continue(_) => (
                entry::tone_of(WaitTone::Done, c),
                entry::tone_ink(WaitTone::Done, c),
            ),
            Kind::Prompt => (c.accent, c.accent_ink),
        }
    }
}

pub fn resolve(chosen: Option<Aim>, waits: &[ScreenWait]) -> Aim {
    match chosen {
        Some(Aim::Prompt) => Aim::Prompt,
        Some(Aim::Alert(id)) if waits.iter().any(|w| w.wait.id == id) => Aim::Alert(id),
        _ => waits.first().map_or(Aim::Prompt, |w| Aim::Alert(w.wait.id)),
    }
}

pub fn kind_of(aim: Aim, waits: &[ScreenWait]) -> Kind {
    let Aim::Alert(id) = aim else {
        return Kind::Prompt;
    };
    match waits.iter().find(|w| w.wait.id == id) {
        Some(w) if w.wait.answerable => match w.tone {
            WaitTone::Ask => Kind::Answer(id),
            WaitTone::Done => Kind::Continue(id),
        },
        _ => Kind::Prompt,
    }
}

pub fn cycle(current: Aim, waits: &[ScreenWait], by: isize) -> Aim {
    let stops: Vec<Aim> = waits
        .iter()
        .map(|w| Aim::Alert(w.wait.id))
        .chain(std::iter::once(Aim::Prompt))
        .collect();
    let at = stops.iter().position(|stop| *stop == current).unwrap_or(0);
    stops[(at as isize + by).rem_euclid(stops.len() as isize) as usize]
}
