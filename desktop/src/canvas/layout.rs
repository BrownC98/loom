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

#[derive(Clone, Debug)]
pub struct Sibling {
    pub key: String,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushSide {
    Right,
    Down,
    Left,
    Up,
}

const PUSH_SIDES: [PushSide; 4] = [
    PushSide::Right,
    PushSide::Down,
    PushSide::Left,
    PushSide::Up,
];

fn clearance(rect: Rect, by: Rect, side: PushSide, gap: f64) -> f64 {
    match side {
        PushSide::Right => by.at.x + by.size.width + gap - rect.at.x,
        PushSide::Down => by.at.y + by.size.height + gap - rect.at.y,
        PushSide::Left => rect.at.x + rect.size.width - (by.at.x - gap),
        PushSide::Up => rect.at.y + rect.size.height - (by.at.y - gap),
    }
}

fn moved(rect: Rect, side: PushSide, distance: f64) -> Rect {
    let (dx, dy) = match side {
        PushSide::Right => (distance, 0.0),
        PushSide::Down => (0.0, distance),
        PushSide::Left => (-distance, 0.0),
        PushSide::Up => (0.0, -distance),
    };
    Rect {
        at: Point {
            x: rect.at.x + dx,
            y: rect.at.y + dy,
        },
        size: rect.size,
    }
}

pub fn push(
    siblings: &[Sibling],
    mover: Rect,
    targets: &[String],
    gap: f64,
) -> BTreeMap<String, Point> {
    push_by(siblings, mover, targets, gap, &|rect| {
        PUSH_SIDES.iter().skip(1).fold(PUSH_SIDES[0], |best, &s| {
            if clearance(rect, mover, s, gap) < clearance(rect, mover, best, gap) {
                s
            } else {
                best
            }
        })
    })
}

fn away(rect: Rect, mover: Rect) -> PushSide {
    if (rect.at.x - mover.at.x) / mover.size.width >= (rect.at.y - mover.at.y) / mover.size.height {
        PushSide::Right
    } else {
        PushSide::Down
    }
}

fn push_by(
    siblings: &[Sibling],
    mover: Rect,
    targets: &[String],
    gap: f64,
    side_of: &dyn Fn(Rect) -> PushSide,
) -> BTreeMap<String, Point> {
    let mut now: Vec<(String, Rect)> = Vec::new();
    for s in siblings {
        match now.iter_mut().find(|(k, _)| *k == s.key) {
            Some(slot) => slot.1 = s.rect,
            None => now.push((s.key.clone(), s.rect)),
        }
    }
    let mut out = BTreeMap::new();
    for key in targets {
        let Some(rect) = now.iter().find(|(k, _)| k == key).map(|(_, r)| *r) else {
            continue;
        };
        if !overlaps(rect, mover) {
            continue;
        }
        let side = side_of(rect);
        let mut queue = std::collections::VecDeque::from([(
            key.clone(),
            moved(rect, side, clearance(rect, mover, side, gap)),
        )]);
        while let Some((k, b)) = queue.pop_front() {
            if let Some(slot) = now.iter_mut().find(|(n, _)| *n == k) {
                slot.1 = b;
            }
            out.insert(k.clone(), b.at);
            for (other, o) in &now {
                if *other != k && overlaps(*o, b) {
                    queue.push_back((other.clone(), moved(*o, side, clearance(*o, b, side, gap))));
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CenterSide {
    pub x: f64,
    pub y: f64,
}

fn center(r: Rect) -> Point {
    Point {
        x: r.at.x + r.size.width / 2.0,
        y: r.at.y + r.size.height / 2.0,
    }
}

fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        v
    }
}

pub fn center_side(dragged: Rect, rect: Rect) -> CenterSide {
    let (d, o) = (center(dragged), center(rect));
    let apart_x = dragged.at.x + dragged.size.width <= rect.at.x
        || rect.at.x + rect.size.width <= dragged.at.x;
    let apart_y = dragged.at.y + dragged.size.height <= rect.at.y
        || rect.at.y + rect.size.height <= dragged.at.y;
    CenterSide {
        x: if apart_x { sign(d.x - o.x) } else { 0.0 },
        y: if apart_y { sign(d.y - o.y) } else { 0.0 },
    }
}

pub fn crossed(before: CenterSide, dragged: Rect, rect: Rect) -> bool {
    let o = center(rect);
    let lead = Point {
        x: if before.x < 0.0 {
            dragged.at.x + dragged.size.width
        } else {
            dragged.at.x
        },
        y: if before.y < 0.0 {
            dragged.at.y + dragged.size.height
        } else {
            dragged.at.y
        },
    };
    (before.x != 0.0 && sign(lead.x - o.x) != before.x)
        || (before.y != 0.0 && sign(lead.y - o.y) != before.y)
}

#[derive(Clone, Debug)]
pub struct Swap {
    pub slot: Rect,
    pub sides: BTreeMap<String, CenterSide>,
    pub at: BTreeMap<String, Point>,
}

pub fn swap(state: &Swap, others: &[Sibling], dragged: Rect) -> Swap {
    let mut slot = state.slot;
    let mut sides = state.sides.clone();
    let mut at = state.at.clone();
    for s in others {
        let o = Rect {
            at: at.get(&s.key).copied().unwrap_or(s.rect.at),
            size: s.rect.size,
        };
        if !overlaps(dragged, o) {
            sides.insert(s.key.clone(), center_side(dragged, o));
            continue;
        }
        let before = sides
            .get(&s.key)
            .copied()
            .unwrap_or_else(|| center_side(slot, o));
        sides.insert(s.key.clone(), before);
        if !crossed(before, dragged, o) {
            continue;
        }
        let edge = |side: f64, now: f64, from: f64, len: f64, own: f64| {
            if side == 0.0 {
                now
            } else if side > 0.0 {
                from + len - own
            } else {
                from
            }
        };
        at.insert(
            s.key.clone(),
            Point {
                x: edge(before.x, o.at.x, slot.at.x, slot.size.width, o.size.width),
                y: edge(before.y, o.at.y, slot.at.y, slot.size.height, o.size.height),
            },
        );
        slot = o;
        sides.insert(
            s.key.clone(),
            CenterSide {
                x: -before.x,
                y: -before.y,
            },
        );
    }
    Swap { slot, sides, at }
}

pub fn unpile(siblings: &[Sibling], gap: f64) -> BTreeMap<String, Point> {
    let mut now: BTreeMap<String, Point> = siblings
        .iter()
        .map(|s| (s.key.clone(), s.rect.at))
        .collect();
    for (i, s) in siblings.iter().enumerate() {
        let boxes: Vec<Sibling> = siblings
            .iter()
            .map(|b| Sibling {
                key: b.key.clone(),
                rect: Rect {
                    at: now[&b.key],
                    size: b.rect.size,
                },
            })
            .collect();
        let mover = boxes[i].rect;
        let later: Vec<String> = boxes[i + 1..]
            .iter()
            .filter(|o| overlaps(o.rect, mover))
            .map(|o| o.key.clone())
            .collect();
        let rest: Vec<Sibling> = boxes.into_iter().filter(|b| b.key != s.key).collect();
        now.extend(push_by(&rest, mover, &later, gap, &|r| away(r, mover)));
    }
    now
}

pub fn same_point(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

pub const SIDES: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];
pub const ARRANGE_DIRECTIONS: [Direction; 4] = [
    Direction::Down,
    Direction::Right,
    Direction::Up,
    Direction::Left,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeSides {
    pub parent: Side,
    pub child: Side,
}

pub fn tree_sides(direction: Direction) -> TreeSides {
    let (parent, child) = match direction {
        Direction::Down => (Side::Bottom, Side::Top),
        Direction::Up => (Side::Top, Side::Bottom),
        Direction::Right => (Side::Right, Side::Left),
        Direction::Left => (Side::Left, Side::Right),
    };
    TreeSides { parent, child }
}

fn ahead_gap(parent: Rect, child: Rect, direction: Direction) -> f64 {
    match direction {
        Direction::Down => child.at.y - (parent.at.y + parent.size.height),
        Direction::Up => parent.at.y - (child.at.y + child.size.height),
        Direction::Right => child.at.x - (parent.at.x + parent.size.width),
        Direction::Left => parent.at.x - (child.at.x + child.size.width),
    }
}

pub fn step_at(parent: Rect, child: Rect, direction: Direction, bend: f64) -> Option<f64> {
    let gap = ahead_gap(parent, child, direction);
    (gap >= 0.0).then(|| bend.min(gap / 2.0))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendCenter {
    pub x: Option<f64>,
    pub y: Option<f64>,
}

pub fn bend_center(parent: Rect, bend: f64, direction: Direction) -> BendCenter {
    let none = BendCenter { x: None, y: None };
    match direction {
        Direction::Down => BendCenter {
            y: Some(parent.at.y + parent.size.height + bend),
            ..none
        },
        Direction::Up => BendCenter {
            y: Some(parent.at.y - bend),
            ..none
        },
        Direction::Right => BendCenter {
            x: Some(parent.at.x + parent.size.width + bend),
            ..none
        },
        Direction::Left => BendCenter {
            x: Some(parent.at.x - bend),
            ..none
        },
    }
}

pub fn edge_sides(parent: Rect, child: Rect, direction: Direction) -> TreeSides {
    if ahead_gap(parent, child, direction) >= 0.0 {
        return tree_sides(direction);
    }
    let gap_right = child.at.x - (parent.at.x + parent.size.width);
    let gap_left = parent.at.x - (child.at.x + child.size.width);
    let gap_down = child.at.y - (parent.at.y + parent.size.height);
    let gap_up = parent.at.y - (child.at.y + child.size.height);
    let horizontal = gap_right.max(gap_left) > gap_down.max(gap_up);
    let toward = if horizontal {
        if gap_right > gap_left {
            Direction::Right
        } else {
            Direction::Left
        }
    } else if gap_down > gap_up {
        Direction::Down
    } else {
        Direction::Up
    };
    tree_sides(toward)
}

#[derive(Clone, Copy, Debug)]
pub struct EdgeEnd {
    pub rect: Rect,
    pub side: Side,
}

pub fn side_point(end: EdgeEnd) -> Point {
    let EdgeEnd { rect, side } = end;
    let mid_x = rect.at.x + rect.size.width / 2.0;
    let mid_y = rect.at.y + rect.size.height / 2.0;
    match side {
        Side::Left => Point {
            x: rect.at.x,
            y: mid_y,
        },
        Side::Right => Point {
            x: rect.at.x + rect.size.width,
            y: mid_y,
        },
        Side::Top => Point {
            x: mid_x,
            y: rect.at.y,
        },
        Side::Bottom => Point {
            x: mid_x,
            y: rect.at.y + rect.size.height,
        },
    }
}

fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

pub fn label_heading(from: EdgeEnd, to: EdgeEnd, step: Option<f64>) -> Direction {
    let a = side_point(from);
    let b = side_point(to);
    let dx = js_round(b.x - a.x);
    let dy = js_round(b.y - a.y);
    let vertical = matches!(from.side, Side::Top | Side::Bottom);
    let lean = match (step.is_some(), vertical) {
        (true, true) => Point { x: dx, y: 0.0 },
        (true, false) => Point { x: 0.0, y: dy },
        (false, true) => Point { x: 2.0 * dx, y: dy },
        (false, false) => Point { x: dx, y: 2.0 * dy },
    };
    let m = if lean.x == 0.0 && lean.y == 0.0 {
        Point { x: dx, y: dy }
    } else {
        lean
    };
    if m.x.abs() > m.y.abs() {
        if m.x > 0.0 {
            Direction::Right
        } else {
            Direction::Left
        }
    } else if m.y > 0.0 {
        Direction::Down
    } else {
        Direction::Up
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

pub fn follow_card(old: Point, now: Point, camera: Camera) -> Camera {
    Camera {
        x: camera.x - (now.x - old.x) * camera.zoom,
        y: camera.y - (now.y - old.y) * camera.zoom,
        zoom: camera.zoom,
    }
}
