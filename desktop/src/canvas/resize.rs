use std::collections::HashMap;

use api::canvas::{CardSize, Placed, Spot};
use api::ids::NodeId;
use gpui::{
    App, CursorStyle, Div, InteractiveElement, MouseButton, MouseDownEvent, Pixels, Point, Styled,
    Window, div, px,
};

use super::layout::{Point as Graph, Rect, Size};
use super::scene::{Card, MIN_CARD, STANDARD, Scene};
use crate::link::Snapshot;

const LINE: f32 = 6.0;
const CORNER: f32 = 12.0;
const MIN_HIT: f32 = 4.0;
const SAME: f64 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const EDGES: [Edge; 8] = [
    Edge::Left,
    Edge::Right,
    Edge::Top,
    Edge::Bottom,
    Edge::TopLeft,
    Edge::TopRight,
    Edge::BottomLeft,
    Edge::BottomRight,
];

impl Edge {
    fn sides(self) -> (i8, i8) {
        match self {
            Edge::Top => (0, -1),
            Edge::Bottom => (0, 1),
            Edge::Left => (-1, 0),
            Edge::Right => (1, 0),
            Edge::TopLeft => (-1, -1),
            Edge::TopRight => (1, -1),
            Edge::BottomLeft => (-1, 1),
            Edge::BottomRight => (1, 1),
        }
    }

    fn cursor(self) -> CursorStyle {
        match self {
            Edge::Left | Edge::Right => CursorStyle::ResizeLeftRight,
            Edge::Top | Edge::Bottom => CursorStyle::ResizeUpDown,
            Edge::TopLeft | Edge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Edge::TopRight | Edge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }

    fn place(self, zoom: f32) -> Div {
        let line = px((LINE * zoom).max(MIN_HIT));
        let corner = px((CORNER * zoom).max(MIN_HIT * 2.0));
        let bar = div().absolute();
        match self {
            Edge::Top => bar.top_0().left(corner).right(corner).h(line),
            Edge::Bottom => bar.bottom_0().left(corner).right(corner).h(line),
            Edge::Left => bar.left_0().top(corner).bottom(corner).w(line),
            Edge::Right => bar.right_0().top(corner).bottom(corner).w(line),
            Edge::TopLeft => bar.top_0().left_0().w(corner).h(corner),
            Edge::TopRight => bar.top_0().right_0().w(corner).h(corner),
            Edge::BottomLeft => bar.bottom_0().left_0().w(corner).h(corner),
            Edge::BottomRight => bar.bottom_0().right_0().w(corner).h(corner),
        }
    }
}

pub fn handles<L>(zoom: f32, down: impl Fn(Edge) -> L) -> Vec<Div>
where
    L: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    EDGES
        .iter()
        .map(|&edge| {
            edge.place(zoom)
                .cursor(edge.cursor())
                .on_mouse_down(MouseButton::Left, down(edge))
        })
        .collect()
}

fn axis(at: f64, length: f64, delta: f64, side: i8, min: f64) -> (f64, f64) {
    match side {
        1 => (at, (length + delta).max(min)),
        -1 => {
            let next = (length - delta).max(min);
            (at + length - next, next)
        }
        _ => (at, length),
    }
}

fn dragged(edge: Edge, start: Rect, dx: f64, dy: f64) -> Rect {
    let (horizontal, vertical) = edge.sides();
    let (x, width) = axis(start.at.x, start.size.width, dx, horizontal, MIN_CARD.width);
    let (y, height) = axis(start.at.y, start.size.height, dy, vertical, MIN_CARD.height);
    Rect {
        at: Graph { x, y },
        size: Size { width, height },
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < SAME
}

struct Active {
    node: u32,
    edge: Edge,
    from: Point<Pixels>,
    start: Rect,
    spot: Graph,
    moves: bool,
}

struct Draft {
    size: Size,
    shift: Graph,
    spot: Option<Graph>,
}

pub struct Saved {
    pub size: CardSize,
    pub spot: Option<Placed>,
}

#[derive(Default)]
pub struct Resizer {
    active: Option<Active>,
    drafts: HashMap<u32, Draft>,
}

fn arranged(snapshot: &Snapshot, node: u32) -> bool {
    snapshot
        .nodes
        .iter()
        .find(|n| n.id.0 == node)
        .is_some_and(|n| snapshot.layout.arranged.contains(&n.place.repo))
}

impl Resizer {
    pub fn begin(&mut self, card: &Card, edge: Edge, from: Point<Pixels>, snapshot: &Snapshot) {
        self.active = Some(Active {
            node: card.node,
            edge,
            from,
            start: card.rect,
            spot: card.spot,
            moves: !arranged(snapshot, card.node),
        });
    }

    pub fn drag(&mut self, to: Point<Pixels>, zoom: f32) -> bool {
        let Some(active) = &self.active else {
            return false;
        };
        let delta = to - active.from;
        let zoom = f64::from(zoom);
        let rect = dragged(
            active.edge,
            active.start,
            f64::from(f32::from(delta.x)) / zoom,
            f64::from(f32::from(delta.y)) / zoom,
        );
        let shift = if active.moves {
            Graph {
                x: rect.at.x - active.start.at.x,
                y: rect.at.y - active.start.at.y,
            }
        } else {
            Graph { x: 0.0, y: 0.0 }
        };
        self.drafts.insert(
            active.node,
            Draft {
                size: rect.size,
                shift,
                spot: None,
            },
        );
        true
    }

    pub fn end(&mut self) -> Option<Saved> {
        let active = self.active.take()?;
        let draft = self.drafts.get_mut(&active.node)?;
        let size = Size {
            width: draft.size.width.round(),
            height: draft.size.height.round(),
        };
        let shift = Graph {
            x: draft.shift.x.round(),
            y: draft.shift.y.round(),
        };
        let moved = shift.x != 0.0 || shift.y != 0.0;
        if size == active.start.size && !moved {
            self.drafts.remove(&active.node);
            return None;
        }
        let spot = moved.then_some(Graph {
            x: active.spot.x + shift.x,
            y: active.spot.y + shift.y,
        });
        draft.size = size;
        draft.shift = shift;
        draft.spot = spot;
        Some(Saved {
            size: CardSize {
                node: NodeId(active.node),
                width: size.width,
                height: size.height,
            },
            spot: spot.map(|at| Placed {
                spot: Spot::Node {
                    node: NodeId(active.node),
                },
                at: api::canvas::Point { x: at.x, y: at.y },
            }),
        })
    }

    pub fn reset(&mut self, nodes: &[u32]) {
        for &node in nodes {
            self.drafts.insert(
                node,
                Draft {
                    size: STANDARD,
                    shift: Graph { x: 0.0, y: 0.0 },
                    spot: None,
                },
            );
        }
    }

    pub fn fail(&mut self, nodes: &[u32]) {
        for node in nodes {
            self.drafts.remove(node);
        }
    }

    pub fn sizes(&self) -> HashMap<u32, Size> {
        self.drafts.iter().map(|(&n, d)| (n, d.size)).collect()
    }

    pub fn place(&self, scene: &mut Scene) {
        for card in &mut scene.cards {
            if let Some(draft) = self.drafts.get(&card.node) {
                card.rect.at.x += draft.shift.x;
                card.rect.at.y += draft.shift.y;
            }
        }
    }

    pub fn settle(&mut self, snapshot: &Snapshot) {
        let active = self.active.as_ref().map(|a| a.node);
        self.drafts.retain(|&node, draft| {
            if active == Some(node) {
                return true;
            }
            if !snapshot.nodes.iter().any(|n| n.id.0 == node) {
                return false;
            }
            let saved = snapshot
                .sizes
                .iter()
                .find(|s| s.node.0 == node)
                .map_or(STANDARD, |s| Size {
                    width: s.width,
                    height: s.height,
                });
            let sized =
                near(saved.width, draft.size.width) && near(saved.height, draft.size.height);
            let placed = draft.spot.is_none_or(|want| {
                snapshot.layout.placed.iter().any(|p| {
                    matches!(&p.spot, Spot::Node { node: n } if n.0 == node)
                        && near(p.at.x, want.x)
                        && near(p.at.y, want.y)
                })
            });
            !(sized && placed)
        });
    }
}
