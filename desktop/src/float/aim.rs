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

/// A stop of the composer: the node it speaks to and what it answers there.
pub type Stop = (u32, Aim);

/// The stop that the human chose, with the place of its wait in the stream.
/// The place lets the composer find the next wait after this one closes.
#[derive(Clone, Copy, Debug)]
pub struct Chosen {
    pub stop: Stop,
    pub place: Option<Place>,
}

/// Time order of a wait in the stream: `since`, then the wait number.
pub type Place = (i64, u32);

/// One open wait of the loom-wide stream, with the node and repository it belongs to.
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

/// All open waits of all nodes, oldest first.
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

/// The stop that is current. A pinned stop wins. Then the stop that the human chose.
/// If that wait closed, the next wait in time order is current (the oldest one after the last).
/// With no choice, the oldest wait is current.
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

/// Steps through the stream; the last stop is the instruction to the current node.
pub fn cycle(current: Stop, stream: &[Streamed], by: isize) -> Stop {
    let stops: Vec<Stop> = stream
        .iter()
        .map(Streamed::stop)
        .chain(std::iter::once((current.0, Aim::Prompt)))
        .collect();
    let at = stops.iter().position(|stop| *stop == current).unwrap_or(0);
    stops[(at as isize + by).rem_euclid(stops.len() as isize) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::graph::waits::HumanWait;

    fn wait(node: u32, id: u32, since: i64) -> Streamed {
        Streamed {
            node,
            name: format!("n{node}"),
            repo: "r".to_owned(),
            wait: ScreenWait {
                wait: HumanWait {
                    id: WaitId(id),
                    kind: api::graph::waits::WaitKind::Question,
                    text: String::new(),
                    since,
                    answerable: true,
                    doc: None,
                    instruction: None,
                    images: Vec::new(),
                    prompt: None,
                    parts: Vec::new(),
                    choices: Vec::new(),
                },
                tone: WaitTone::Ask,
                summary: String::new(),
            },
        }
    }

    fn alert(node: u32, id: u32) -> Stop {
        (node, Aim::Alert(WaitId(id)))
    }

    #[test]
    fn default_is_the_oldest_wait_of_any_node() {
        let stream = [wait(2, 7, 10), wait(1, 8, 20)];
        assert_eq!(resolve(None, None, &stream, Some(1)), Some(alert(2, 7)));
        assert_eq!(resolve(None, None, &[], Some(1)), Some((1, Aim::Prompt)));
    }

    #[test]
    fn closed_wait_hands_over_to_the_next_in_time_order() {
        let stream = [wait(1, 7, 10), wait(2, 9, 30)];
        let gone = Chosen {
            stop: alert(1, 8),
            place: Some((20, 8)),
        };
        assert_eq!(resolve(None, Some(gone), &stream, None), Some(alert(2, 9)));
        let last = Chosen {
            stop: alert(2, 11),
            place: Some((40, 11)),
        };
        assert_eq!(resolve(None, Some(last), &stream, None), Some(alert(1, 7)));
        assert_eq!(resolve(None, Some(last), &[], None), Some((2, Aim::Prompt)));
    }

    #[test]
    fn cycle_runs_through_the_stream_and_ends_at_the_prompt() {
        let stream = [wait(1, 7, 10), wait(2, 9, 30)];
        assert_eq!(cycle(alert(1, 7), &stream, 1), alert(2, 9));
        assert_eq!(cycle(alert(2, 9), &stream, 1), (2, Aim::Prompt));
        assert_eq!(cycle((2, Aim::Prompt), &stream, 1), alert(1, 7));
        assert_eq!(cycle(alert(1, 7), &stream, -1), (1, Aim::Prompt));
    }
}
