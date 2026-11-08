use api::graph::waits::{ScreenWait, WaitTone};
use api::ids::WaitId;
use gpui::Rgba;

use crate::link::Snapshot;
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

pub type Stop = (u32, Aim);

#[derive(Clone, Copy, Debug)]
pub struct Chosen {
    pub stop: Stop,
    pub place: Option<Place>,
}

pub type Place = (i64, u32);

#[derive(Clone)]
pub struct Streamed {
    pub node: u32,
    pub name: String,
    pub repo: String,
    pub wait: ScreenWait,
}

impl Streamed {
    pub fn place(&self) -> Place {
        (self.wait.wait.since, self.wait.wait.id.0)
    }

    pub fn stop(&self) -> Stop {
        (self.node, Aim::Alert(self.wait.wait.id))
    }
}

pub fn stream_of(snapshot: &Snapshot) -> Vec<Streamed> {
    let mut stream: Vec<Streamed> = snapshot
        .nodes
        .iter()
        .flat_map(|n| {
            let repo = snapshot
                .repos
                .iter()
                .find(|r| r.id == n.place.repo)
                .map(|r| r.name.clone())
                .unwrap_or_default();
            n.waits.iter().map(move |wait| Streamed {
                node: n.id.0,
                name: n.name.clone(),
                repo: repo.clone(),
                wait: wait.clone(),
            })
        })
        .collect();
    stream.sort_by_key(Streamed::place);
    stream
}

pub fn chosen(stop: Stop, stream: &[Streamed]) -> Chosen {
    let place = match stop.1 {
        Aim::Alert(id) => stream
            .iter()
            .find(|s| s.wait.wait.id == id)
            .map(Streamed::place),
        Aim::Prompt => None,
    };
    Chosen { stop, place }
}

pub fn resolve(
    held: Option<Stop>,
    chosen: Option<Chosen>,
    stream: &[Streamed],
    fallback: Option<u32>,
) -> Option<Stop> {
    if let Some(stop) = held {
        return Some(stop);
    }
    if let Some(Chosen { stop, place }) = chosen {
        match stop.1 {
            Aim::Prompt => return Some(stop),
            Aim::Alert(id) => {
                if let Some(found) = stream.iter().find(|s| s.wait.wait.id == id) {
                    return Some(found.stop());
                }
                let next = place
                    .and_then(|at| stream.iter().find(|s| s.place() > at))
                    .or(stream.first());
                return Some(next.map_or((stop.0, Aim::Prompt), Streamed::stop));
            }
        }
    }
    stream
        .first()
        .map(Streamed::stop)
        .or(fallback.map(|node| (node, Aim::Prompt)))
}

pub fn kind_of(aim: Aim, stream: &[Streamed]) -> Kind {
    let Aim::Alert(id) = aim else {
        return Kind::Prompt;
    };
    match stream.iter().find(|s| s.wait.wait.id == id) {
        Some(s) if s.wait.wait.answerable => match s.wait.tone {
            WaitTone::Ask => Kind::Answer(id),
            WaitTone::Done => Kind::Continue(id),
        },
        _ => Kind::Prompt,
    }
}

pub fn cycle(current: Stop, stream: &[Streamed], by: isize) -> Stop {
    let stops: Vec<Stop> = stream
        .iter()
        .map(Streamed::stop)
        .chain(std::iter::once((current.0, Aim::Prompt)))
        .collect();
    let at = stops.iter().position(|stop| *stop == current).unwrap_or(0);
    stops[(at as isize + by).rem_euclid(stops.len() as isize) as usize]
}
