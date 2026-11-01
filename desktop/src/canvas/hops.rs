#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::HashMap;

use super::layout::{Direction, Point, Rect, Size};

const FULL_TURN: f64 = 360.0;
const HUE_TRIES: usize = 12;
const END_GAP: f64 = 40.0;
const LABEL_SPOTS: [f64; 7] = [0.5, 0.3, 0.7, 0.15, 0.85, 0.05, 0.95];
const LABEL_GAP: f64 = 2.0;
const BEND_GAP: f64 = 16.0;
const BOX_GAP: f64 = 8.0;

#[derive(Clone, Copy, Debug)]
pub struct HopEnd {
    pub id: u32,
    pub rect: Rect,
    pub room: Rect,
    pub bend: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct LabelSizes {
    pub full: Size,
    pub short: Size,
}

#[derive(Clone, Debug)]
pub struct HopPair {
    pub key: String,
    pub source: HopEnd,
    pub target: HopEnd,
    pub label: LabelSizes,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HopLabel {
    pub at: Point,
    pub short: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HopRoute {
    pub points: [Point; 4],
    pub label: Option<HopLabel>,
    pub turned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HopEndName {
    Source,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HopPart {
    pub from: HopEndName,
    pub to: HopEndName,
    pub back: bool,
}

pub fn hop_parts(
    counts: [u32; 2],
    source: Option<Point>,
    target: Option<Point>,
    turned: bool,
) -> Vec<HopPart> {
    let source_first = match (source, target) {
        (Some(s), Some(t)) => {
            if turned {
                s.y >= t.y
            } else {
                s.x <= t.x
            }
        }
        _ => true,
    };
    let (first, second) = if source_first {
        (HopEndName::Source, HopEndName::Target)
    } else {
        (HopEndName::Target, HopEndName::Source)
    };
    let count = |name: HopEndName| counts[name as usize];
    [
        HopPart {
            from: first,
            to: second,
            back: false,
        },
        HopPart {
            from: second,
            to: first,
            back: true,
        },
    ]
    .into_iter()
    .filter(|part| count(part.to) > 0)
    .collect()
}

pub fn pick_hue(shown: &[f64], mut random: impl FnMut() -> f64) -> f64 {
    let gap = |hue: f64| {
        shown.iter().fold(FULL_TURN, |least, s| {
            let apart = (hue - s).abs();
            least.min(apart.min(FULL_TURN - apart))
        })
    };
    let tries: Vec<f64> = (0..HUE_TRIES)
        .map(|_| (random() * FULL_TURN).floor())
        .collect();
    tries
        .into_iter()
        .reduce(|best, hue| if gap(hue) > gap(best) { hue } else { best })
        .unwrap_or(0.0)
}

#[derive(Clone, Copy)]
struct Edges {
    l: f64,
    t: f64,
    r: f64,
    b: f64,
}

fn to_down(direction: Direction, p: Point) -> Point {
    match direction {
        Direction::Down => p,
        Direction::Up => Point { x: p.x, y: -p.y },
        Direction::Right => Point { x: p.y, y: p.x },
        Direction::Left => Point { x: p.y, y: -p.x },
    }
}

fn from_down(direction: Direction, p: Point) -> Point {
    match direction {
        Direction::Down => p,
        Direction::Up => Point { x: p.x, y: -p.y },
        Direction::Right => Point { x: p.y, y: p.x },
        Direction::Left => Point { x: -p.y, y: p.x },
    }
}

fn rect_down(rect: Rect, direction: Direction) -> Edges {
    let a = to_down(direction, rect.at);
    let b = to_down(
        direction,
        Point {
            x: rect.at.x + rect.size.width,
            y: rect.at.y + rect.size.height,
        },
    );
    Edges {
        l: a.x.min(b.x),
        t: a.y.min(b.y),
        r: a.x.max(b.x),
        b: a.y.max(b.y),
    }
}

fn apart(r: Edges, q: Edges, gap: f64) -> bool {
    r.r + gap <= q.l || q.r + gap <= r.l || r.b + gap <= q.t || q.b + gap <= r.t
}

fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn then(first: Ordering, second: impl FnOnce() -> Ordering) -> Ordering {
    if first == Ordering::Equal {
        second()
    } else {
        first
    }
}

fn cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

struct Shape {
    ids: Vec<u32>,
    rects: HashMap<u32, Edges>,
    rooms: HashMap<u32, Edges>,
}

pub fn sibling_lanes(
    parent: Rect,
    pairs: &[HopPair],
    direction: Direction,
) -> Vec<(String, HopRoute)> {
    let up = rect_down(parent, direction);
    let mut shape = Shape {
        ids: Vec::new(),
        rects: HashMap::new(),
        rooms: HashMap::new(),
    };
    for pair in pairs {
        for end in [&pair.source, &pair.target] {
            if shape
                .rects
                .insert(end.id, rect_down(end.rect, direction))
                .is_none()
            {
                shape.ids.push(end.id);
            }
            shape.rooms.insert(end.id, rect_down(end.room, direction));
        }
    }
    let rects = &shape.rects;
    let mid = |id: u32| (rects[&id].l + rects[&id].r) / 2.0;
    let room = pairs
        .iter()
        .map(|p| p.label.full.height)
        .fold(f64::NEG_INFINITY, f64::max)
        + LABEL_GAP;
    let bus = |end: &HopEnd| up.b + end.bend.unwrap_or((rects[&end.id].t - up.b) / 2.0);
    let bend_line = |p: &HopPair| bus(&p.source).max(bus(&p.target));
    let head_top = |p: &HopPair| rects[&p.source.id].t.min(rects[&p.target.id].t) - LABEL_GAP;
    let wanted = |p: &HopPair| bend_line(p) + BEND_GAP.max(room / 2.0);
    let bus_line = |p: &HopPair| {
        if head_top(p) >= wanted(p) {
            wanted(p)
        } else {
            (bend_line(p) + head_top(p)) / 2.0
        }
    };
    let labelled = |p: &HopPair| head_top(p) - bus_line(p) >= room / 2.0;
    let band = |p: &HopPair| {
        (
            bus_line(p),
            if labelled(p) {
                head_top(p) - room / 2.0
            } else {
                head_top(p)
            },
        )
    };
    let mut order: Vec<&HopPair> = pairs
        .iter()
        .filter(|p| head_top(p) >= bus_line(p))
        .collect();
    if order.is_empty() {
        return Vec::new();
    }
    let span = |p: &HopPair| (mid(p.source.id) - mid(p.target.id)).abs();
    let left = |p: &HopPair| mid(p.source.id).min(mid(p.target.id));
    order.sort_by(|a, b| {
        then(cmp(span(a), span(b)), || {
            then(cmp(left(a), left(b)), || a.key.cmp(&b.key))
        })
    });

    struct End {
        pair: usize,
        toward: f64,
    }
    let mut ends: HashMap<u32, Vec<End>> = HashMap::new();
    for (i, p) in order.iter().enumerate() {
        for (end, other) in [(&p.source, &p.target), (&p.target, &p.source)] {
            ends.entry(end.id).or_default().push(End {
                pair: i,
                toward: sign(mid(other.id) - mid(end.id)),
            });
        }
    }
    let mut at: HashMap<(usize, u32), f64> = HashMap::new();
    for (&id, list) in ends.iter_mut() {
        list.sort_by(|a, b| {
            then(cmp(a.toward, b.toward), || {
                if a.toward < 0.0 {
                    a.pair.cmp(&b.pair)
                } else {
                    b.pair.cmp(&a.pair)
                }
            })
        });
        let r = rects[&id];
        let gap = END_GAP.min((r.r - r.l) / (list.len() as f64 + 1.0));
        for (k, e) in list.iter().enumerate() {
            at.insert(
                (e.pair, id),
                mid(id) + (k as f64 - (list.len() as f64 - 1.0) / 2.0) * gap,
            );
        }
    }
    let reach = |i: usize| {
        let xa = at[&(i, order[i].source.id)];
        let xb = at[&(i, order[i].target.id)];
        (xa.min(xb), xa.max(xb))
    };
    let mut boxes: Vec<(Edges, f64)> = shape
        .ids
        .iter()
        .filter(|id| shape.rooms[id].t < rects[id].t)
        .map(|id| (shape.rooms[id], rects[id].t - LABEL_GAP - room))
        .collect();
    boxes.sort_by(|a, b| cmp(b.0.t, a.0.t));
    let mut lanes: Vec<f64> = Vec::new();
    for (i, p) in order.iter().enumerate() {
        let x = reach(i);
        let (lo, hi) = band(p);
        let mut y = (0..lanes.len())
            .filter(|&j| reach(j).0 < x.1 && x.0 < reach(j).1)
            .fold(hi, |y, j| y.min(lanes[j] - room));
        for (q, card) in &boxes {
            if q.l < x.1 && x.0 < q.r && y > q.t - BOX_GAP && y < *card {
                y = q.t - BOX_GAP;
            }
        }
        lanes.push(lo.max(y));
    }

    let heads = pairs
        .iter()
        .flat_map(|p| [&p.source, &p.target])
        .filter(|e| e.room.at.y < e.rect.at.y)
        .map(|e| {
            rect_down(
                Rect {
                    at: e.room.at,
                    size: Size {
                        width: e.room.size.width,
                        height: e.rect.at.y - e.room.at.y - LABEL_GAP - room,
                    },
                },
                direction,
            )
        });
    let mut walls: Vec<Edges> = vec![up];
    walls.extend(shape.ids.iter().map(|id| rects[id]));
    walls.extend(heads);
    let mut placed: Vec<Edges> = Vec::new();
    let mut stems: Vec<(usize, Edges)> = Vec::new();
    for (i, p) in order.iter().enumerate() {
        for id in [p.source.id, p.target.id] {
            let x = at[&(i, id)];
            stems.push((
                i,
                Edges {
                    l: x - 1.0,
                    t: lanes[i],
                    r: x + 1.0,
                    b: rects[&id].t,
                },
            ));
        }
    }
    let mut routes = Vec::new();
    for (i, p) in order.iter().enumerate() {
        let y = lanes[i];
        let xa = at[&(i, p.source.id)];
        let xb = at[&(i, p.target.id)];
        let fit = |size: Size, placed: &[Edges]| {
            if !labelled(p) {
                return None;
            }
            let at_spot = |t: f64| {
                let x = xa + (xb - xa) * t;
                Edges {
                    l: x - size.width / 2.0,
                    t: y - size.height / 2.0,
                    r: x + size.width / 2.0,
                    b: y + size.height / 2.0,
                }
            };
            let clear = |r: Edges| {
                walls.iter().all(|&q| apart(r, q, 0.0))
                    && placed.iter().all(|&q| apart(r, q, LABEL_GAP))
                    && stems.iter().all(|&(pair, s)| pair == i || apart(r, s, 0.0))
            };
            LABEL_SPOTS.iter().map(|&t| at_spot(t)).find(|&b| clear(b))
        };
        let full = fit(p.label.full, &placed);
        let spot = full.or_else(|| fit(p.label.short, &placed));
        if let Some(s) = spot {
            placed.push(s);
        }
        let back = |p: Point| from_down(direction, p);
        routes.push((
            p.key.clone(),
            HopRoute {
                points: [
                    back(Point {
                        x: xa,
                        y: rects[&p.source.id].t,
                    }),
                    back(Point { x: xa, y }),
                    back(Point { x: xb, y }),
                    back(Point {
                        x: xb,
                        y: rects[&p.target.id].t,
                    }),
                ],
                label: spot.map(|s| HopLabel {
                    at: back(Point {
                        x: (s.l + s.r) / 2.0,
                        y,
                    }),
                    short: full.is_none(),
                }),
                turned: matches!(direction, Direction::Right | Direction::Left),
            },
        ));
    }
    routes
}
