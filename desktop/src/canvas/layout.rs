#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

pub type NodeId = u32;

const CARD_LANE: f64 = 20.0;
pub const GROUP_GAP: f64 = 32.0;
pub const FREE_GAP: f64 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub at: Point,
    pub size: Size,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Down,
    Right,
    Up,
    Left,
}

#[derive(Clone, Copy, Debug)]
pub struct Inset {
    pub top: f64,
    pub side: f64,
}

#[derive(Clone, Debug)]
pub struct NestGroup {
    pub parent: Option<usize>,
    pub card: Option<NodeId>,
    pub members: Vec<NodeId>,
    pub empty: Size,
}

#[derive(Clone, Copy, Debug)]
pub enum Kid {
    Card(NodeId),
    Group(usize),
}

pub struct Nested {
    pub cards: HashMap<NodeId, Point>,
    pub boxes: Vec<Rect>,
}

#[derive(Clone, Copy)]
struct Edge {
    at: f64,
    boxed: bool,
}

#[derive(Clone, Copy)]
struct Span {
    lo: f64,
    hi: f64,
    r0: usize,
    r1: usize,
}

#[derive(Clone)]
struct Shape {
    at: HashMap<NodeId, f64>,
    boxes: HashMap<usize, Span>,
    lo: BTreeMap<usize, Edge>,
    hi: BTreeMap<usize, Edge>,
}

impl Shape {
    fn shifted(&self, by: f64) -> Shape {
        let slide = |edges: &BTreeMap<usize, Edge>| {
            edges
                .iter()
                .map(|(&r, e)| {
                    (
                        r,
                        Edge {
                            at: e.at + by,
                            boxed: e.boxed,
                        },
                    )
                })
                .collect()
        };
        Shape {
            at: self.at.iter().map(|(&k, x)| (k, x + by)).collect(),
            boxes: self
                .boxes
                .iter()
                .map(|(&k, b)| {
                    (
                        k,
                        Span {
                            lo: b.lo + by,
                            hi: b.hi + by,
                            ..*b
                        },
                    )
                })
                .collect(),
            lo: slide(&self.lo),
            hi: slide(&self.hi),
        }
    }

    fn joined(&self, other: &Shape) -> Shape {
        let merged =
            |a: &BTreeMap<usize, Edge>, b: &BTreeMap<usize, Edge>, outer: fn(f64, f64) -> bool| {
                let mut out = a.clone();
                for (&k, &e) in b {
                    if out.get(&k).is_none_or(|old| outer(e.at, old.at)) {
                        out.insert(k, e);
                    }
                }
                out
            };
        let mut at = self.at.clone();
        at.extend(other.at.iter().map(|(&k, &x)| (k, x)));
        let mut boxes = self.boxes.clone();
        boxes.extend(other.boxes.iter().map(|(&k, &b)| (k, b)));
        Shape {
            at,
            boxes,
            lo: merged(&self.lo, &other.lo, |x, y| x < y),
            hi: merged(&self.hi, &other.hi, |x, y| x > y),
        }
    }

    fn right(&self) -> f64 {
        self.hi
            .values()
            .map(|e| e.at)
            .fold(f64::NEG_INFINITY, f64::max)
    }

    fn own_row(&self) -> f64 {
        if self.hi.len() == 1 {
            f64::NEG_INFINITY
        } else {
            self.right()
        }
    }
}

fn pack(shapes: Vec<Shape>, claim: fn(&Shape) -> f64) -> Option<Shape> {
    let mut acc: Option<Shape> = None;
    let mut right = f64::NEG_INFINITY;
    for s in shapes {
        let Some(placed) = acc.take() else {
            right = claim(&s);
            acc = Some(s);
            continue;
        };
        let deepest = placed.hi.keys().copied().max().unwrap_or(0);
        let mut by = f64::NEG_INFINITY;
        for (&r, lo) in &s.lo {
            let edge = placed.hi.get(&r).copied().or_else(|| {
                (r > deepest && right > f64::NEG_INFINITY).then_some(Edge {
                    at: right,
                    boxed: true,
                })
            });
            if let Some(edge) = edge {
                let gap = if edge.boxed || lo.boxed {
                    GROUP_GAP
                } else {
                    CARD_LANE
                };
                by = by.max(edge.at + gap - lo.at);
            }
        }
        let moved = s.shifted(if by == f64::NEG_INFINITY { 0.0 } else { by });
        right = right.max(claim(&moved));
        acc = Some(placed.joined(&moved));
    }
    acc
}

fn ahead(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

struct Nest<'a> {
    groups: &'a [NestGroup],
    parent_of: &'a dyn Fn(NodeId) -> Option<NodeId>,
    size: &'a dyn Fn(NodeId) -> Size,
    rank: &'a dyn Fn(Kid) -> f64,
    vertical: bool,
    reversed: bool,
    pad_lo: f64,
    pad_hi: f64,
    pad_near: f64,
    pad_far: f64,
    group_of: HashMap<NodeId, usize>,
    rows: RefCell<HashMap<NodeId, usize>>,
}

impl Nest<'_> {
    fn across(&self, s: Size) -> f64 {
        if self.vertical { s.width } else { s.height }
    }

    fn along(&self, s: Size) -> f64 {
        if self.vertical { s.height } else { s.width }
    }

    fn by_rank(&self, mut kids: Vec<Kid>) -> Vec<Kid> {
        kids.sort_by(|&a, &b| ahead((self.rank)(a), (self.rank)(b)));
        kids
    }

    fn kid_cards(&self, id: NodeId) -> Vec<NodeId> {
        self.groups[self.group_of[&id]]
            .members
            .iter()
            .copied()
            .filter(|&k| (self.parent_of)(k) == Some(id))
            .collect()
    }

    fn kid_groups(&self, id: NodeId) -> Vec<usize> {
        (0..self.groups.len())
            .filter(|&i| self.groups[i].card == Some(id))
            .collect()
    }

    fn nested_in(&self, i: usize) -> Vec<usize> {
        (0..self.groups.len())
            .filter(|&k| self.groups[k].parent == Some(i))
            .collect()
    }

    fn roots(&self, i: usize) -> Vec<NodeId> {
        self.groups[i]
            .members
            .iter()
            .copied()
            .filter(|&id| match (self.parent_of)(id) {
                None => true,
                Some(up) => self.group_of.get(&up) != Some(&i),
            })
            .collect()
    }

    fn row_of(&self, id: NodeId) -> usize {
        if let Some(&r) = self.rows.borrow().get(&id) {
            return r;
        }
        let i = self.group_of[&id];
        let up = (self.parent_of)(id);
        let card = match up {
            Some(u) if self.group_of.get(&u) == Some(&i) => Some(u),
            _ => self.groups[i].card,
        };
        let r = card.map_or(0, |c| self.row_of(c) + 1);
        self.rows.borrow_mut().insert(id, r);
        r
    }

    fn lay_card(&self, id: NodeId) -> Shape {
        let r = self.row_of(id);
        let kids: Vec<Kid> = self
            .kid_cards(id)
            .into_iter()
            .map(Kid::Card)
            .chain(self.kid_groups(id).into_iter().map(Kid::Group))
            .collect();
        let shapes = self
            .by_rank(kids)
            .into_iter()
            .map(|k| match k {
                Kid::Card(card) => self.lay_card(card),
                Kid::Group(group) => self.lay_group(group),
            })
            .collect();
        let kids = pack(shapes, Shape::right);
        let own = self.across((self.size)(id));
        let x = kids.as_ref().map_or(0.0, |k| {
            (k.lo[&(r + 1)].at + k.hi[&(r + 1)].at) / 2.0 - own / 2.0
        });
        let me = Shape {
            at: HashMap::from([(id, x)]),
            boxes: HashMap::new(),
            lo: BTreeMap::from([(
                r,
                Edge {
                    at: x,
                    boxed: false,
                },
            )]),
            hi: BTreeMap::from([(
                r,
                Edge {
                    at: x + own,
                    boxed: false,
                },
            )]),
        };
        match kids {
            Some(k) => k.joined(&me),
            None => me,
        }
    }

    fn lay_group(&self, i: usize) -> Shape {
        let group = &self.groups[i];
        if group.members.is_empty() {
            let w = self.across(group.empty);
            return Shape {
                at: HashMap::new(),
                boxes: HashMap::from([(
                    i,
                    Span {
                        lo: 0.0,
                        hi: w,
                        r0: 0,
                        r1: 0,
                    },
                )]),
                lo: BTreeMap::from([(
                    0,
                    Edge {
                        at: 0.0,
                        boxed: true,
                    },
                )]),
                hi: BTreeMap::from([(0, Edge { at: w, boxed: true })]),
            };
        }
        let claim: fn(&Shape) -> f64 = if group.card.is_none() {
            Shape::own_row
        } else {
            Shape::right
        };
        let mut s = pack(
            self.roots(i)
                .into_iter()
                .map(|id| self.lay_card(id))
                .collect(),
            claim,
        )
        .expect("group has members");
        let r0 = *s.lo.keys().next().expect("rows");
        let r1 = *s.lo.keys().next_back().expect("rows");
        let lo = s.lo.values().map(|e| e.at).fold(f64::INFINITY, f64::min) - self.pad_lo;
        let hi =
            s.hi.values()
                .map(|e| e.at)
                .fold(f64::NEG_INFINITY, f64::max)
                + self.pad_hi;
        s.boxes.insert(i, Span { lo, hi, r0, r1 });
        for r in r0..=r1 {
            s.lo.insert(
                r,
                Edge {
                    at: lo,
                    boxed: true,
                },
            );
            s.hi.insert(
                r,
                Edge {
                    at: hi,
                    boxed: true,
                },
            );
        }
        s
    }

    fn linked(&self, i: usize) -> bool {
        self.groups[i]
            .members
            .iter()
            .any(|&id| !self.kid_groups(id).is_empty())
    }
}

pub fn arrange_nested(
    groups: &[NestGroup],
    parent_of: &dyn Fn(NodeId) -> Option<NodeId>,
    size: &dyn Fn(NodeId) -> Size,
    direction: Direction,
    inset: Inset,
    rank: &dyn Fn(Kid) -> f64,
) -> Nested {
    let vertical = matches!(direction, Direction::Down | Direction::Up);
    let reversed = matches!(direction, Direction::Up | Direction::Left);
    let nest = Nest {
        groups,
        parent_of,
        size,
        rank,
        vertical,
        reversed,
        pad_lo: if vertical { inset.side } else { inset.top },
        pad_hi: inset.side,
        pad_near: if direction == Direction::Down {
            inset.top
        } else {
            inset.side
        },
        pad_far: if direction == Direction::Up {
            inset.top
        } else {
            inset.side
        },
        group_of: groups
            .iter()
            .enumerate()
            .flat_map(|(i, g)| g.members.iter().map(move |&id| (id, i)))
            .collect(),
        rows: RefCell::new(HashMap::new()),
    };
    let mut top: Vec<usize> = (0..groups.len())
        .filter(|&i| groups[i].card.is_none())
        .collect();
    top.sort_by(|&a, &b| {
        ahead(rank(Kid::Group(a)), rank(Kid::Group(b)))
            .then((!nest.linked(a)).cmp(&!nest.linked(b)))
    });
    let Some(all) = pack(
        top.iter().map(|&i| nest.lay_group(i)).collect(),
        Shape::right,
    ) else {
        return Nested {
            cards: HashMap::new(),
            boxes: Vec::new(),
        };
    };
    let min = all
        .boxes
        .values()
        .map(|b| b.lo)
        .fold(f64::INFINITY, f64::min);

    let cards: Vec<NodeId> = groups
        .iter()
        .flat_map(|g| g.members.iter().copied())
        .collect();
    let last = cards.iter().map(|&id| nest.row_of(id)).max().unwrap_or(0);
    let row_size: Vec<f64> = (0..=last)
        .map(|r| {
            cards
                .iter()
                .filter(|&&id| nest.row_of(id) == r)
                .map(|&id| nest.along(size(id)))
                .fold(0.0, f64::max)
        })
        .collect();
    let end_row = |i: usize| all.boxes[&i].r1;
    fn closing(nest: &Nest, end_row: &dyn Fn(usize) -> usize, i: usize) -> usize {
        1 + nest
            .nested_in(i)
            .into_iter()
            .filter(|&k| end_row(k) == end_row(i))
            .map(|k| closing(nest, end_row, k))
            .max()
            .unwrap_or(0)
    }
    let close = |r: usize| {
        (0..groups.len())
            .filter(|&i| end_row(i) == r)
            .map(|i| closing(&nest, &end_row, i))
            .fold(1, usize::max)
    };
    let mut start: Vec<f64> = Vec::new();
    for r in 0..=last {
        let at = if r == 0 {
            nest.pad_near
        } else {
            start[r - 1]
                + row_size[r - 1]
                + nest.pad_far * close(r - 1) as f64
                + GROUP_GAP
                + nest.pad_near
        };
        start.push(at);
    }
    fn end(nest: &Nest, start: &[f64], i: usize) -> f64 {
        let g = &nest.groups[i];
        if g.members.is_empty() {
            return nest.along(g.empty);
        }
        let own = g
            .members
            .iter()
            .map(|&id| start[nest.row_of(id)] + nest.along((nest.size)(id)));
        let inner = nest.nested_in(i).into_iter().map(|k| end(nest, start, k));
        own.chain(inner).fold(f64::NEG_INFINITY, f64::max) + nest.pad_far
    }
    let total = top
        .iter()
        .map(|&i| end(&nest, &start, i))
        .fold(f64::NEG_INFINITY, f64::max);
    let place = |a: f64, length: f64, c: f64| {
        let t = if reversed { total - a - length } else { a };
        if vertical {
            Point { x: c, y: t }
        } else {
            Point { x: t, y: c }
        }
    };
    Nested {
        cards: cards
            .iter()
            .map(|&id| {
                (
                    id,
                    place(
                        start[nest.row_of(id)],
                        nest.along(size(id)),
                        all.at[&id] - min,
                    ),
                )
            })
            .collect(),
        boxes: groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let b = all.boxes[&i];
                let t = if g.members.is_empty() {
                    0.0
                } else {
                    start[b.r0] - nest.pad_near
                };
                let e = end(&nest, &start, i);
                let at = place(t, e - t, b.lo - min);
                let size = if vertical {
                    Size {
                        width: b.hi - b.lo,
                        height: e - t,
                    }
                } else {
                    Size {
                        width: e - t,
                        height: b.hi - b.lo,
                    }
                };
                Rect { at, size }
            })
            .collect(),
    }
}

pub fn overlaps(a: Rect, b: Rect) -> bool {
    a.at.x < b.at.x + b.size.width
        && b.at.x < a.at.x + a.size.width
        && a.at.y < b.at.y + b.size.height
        && b.at.y < a.at.y + a.size.height
}

pub fn free_spot(rect: Rect, others: &[Rect], gap: f64) -> Point {
    let mut at = rect.at;
    while let Some(hit) = others.iter().find(|o| {
        overlaps(
            Rect {
                at,
                size: rect.size,
            },
            **o,
        )
    }) {
        at = Point {
            x: hit.at.x + hit.size.width + gap,
            y: at.y,
        };
    }
    at
}

pub struct SettleItem {
    pub key: String,
    pub size: Size,
    pub lead: Point,
    pub auto: Point,
}

pub fn settle(
    items: &[SettleItem],
    placed: &dyn Fn(&str) -> Option<Point>,
    gap: f64,
) -> BTreeMap<String, Point> {
    let mut at = BTreeMap::new();
    let mut taken: Vec<Rect> = Vec::new();
    for item in items {
        if let Some(saved) = placed(&item.key) {
            let p = Point {
                x: saved.x + item.lead.x,
                y: saved.y + item.lead.y,
            };
            at.insert(item.key.clone(), p);
            taken.push(Rect {
                at: p,
                size: item.size,
            });
        }
    }
    for item in items {
        if at.contains_key(&item.key) {
            continue;
        }
        let p = free_spot(
            Rect {
                at: item.auto,
                size: item.size,
            },
            &taken,
            gap,
        );
        at.insert(item.key.clone(), p);
        taken.push(Rect {
            at: p,
            size: item.size,
        });
    }
    at
}
