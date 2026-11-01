use std::collections::{BTreeMap, HashMap, HashSet};

use api::canvas::Spot;

use super::layout::{
    self, Direction, FREE_GAP, GROUP_GAP, Inset, Kid, NestGroup, Nested, NodeId, Point, Rect,
    SettleItem, Size,
};
use super::repos::{Group, GroupKind, attach_cards, group_parents, groups_of, split_chips};
use api::graph::{CardView, NodeView};
use api::repos::RepoView;

use crate::link::Snapshot;

const DIRECTION: Direction = Direction::Down;
const CARD_X: f64 = 20.0;
const CARD_TOP: f64 = 60.0;
const CHIP_ROW: f64 = 43.0;
const EMPTY_GROUP: Size = Size {
    width: 320.0,
    height: 100.0,
};
const REPO_INSET: Inset = Inset {
    top: 62.0,
    side: 16.0,
};
const GROUP_POSITION: Point = Point { x: 16.0, y: 64.0 };
pub const STANDARD: Size = Size {
    width: 600.0,
    height: 466.0,
};
pub const SIMPLE: Size = Size {
    width: 260.0,
    height: 134.0,
};
pub const MIN_CARD: Size = Size {
    width: 360.0,
    height: 200.0,
};

pub struct Frame {
    pub label: String,
    pub rect: Rect,
}

pub struct Card {
    pub node: NodeId,
    pub rect: Rect,
    pub inset: Inset,
}

#[derive(Default)]
pub struct Scene {
    pub direction: Direction,
    pub repos: Vec<Frame>,
    pub groups: Vec<Frame>,
    pub cards: Vec<Card>,
}

struct Laid {
    group: usize,
    members: Vec<NodeId>,
    local: BTreeMap<String, Point>,
    lead: Point,
    size: Size,
    kids: Vec<Laid>,
}

struct Placement<'a> {
    spots: &'a HashMap<String, Point>,
    arranged: bool,
}

impl Placement<'_> {
    fn at(&self, key: &str) -> Option<Point> {
        if self.arranged {
            None
        } else {
            self.spots.get(key).copied()
        }
    }
}

struct Level<'a> {
    groups: Vec<Group>,
    members: Vec<Vec<&'a NodeView>>,
    nest: Vec<NestGroup>,
    tree: Nested,
    chips: HashSet<String>,
    size_of: &'a dyn Fn(NodeId) -> Size,
    placement: Placement<'a>,
}

impl Level<'_> {
    fn inset(&self, path: &str) -> Inset {
        Inset {
            top: CARD_TOP
                + if self.chips.contains(path) {
                    CHIP_ROW
                } else {
                    0.0
                },
            side: CARD_X,
        }
    }

    fn lay(&self, i: usize) -> Laid {
        let base = self.tree.boxes[i].at;
        let rel = |p: Point| Point {
            x: p.x - base.x,
            y: p.y - base.y,
        };
        let kids: Vec<(Laid, Point)> = self
            .nest
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent == Some(i))
            .map(|(k, _)| (self.lay(k), self.tree.boxes[k].at))
            .collect();
        let items: Vec<SettleItem> = self.members[i]
            .iter()
            .map(|v| SettleItem {
                key: v.id.to_string(),
                size: (self.size_of)(v.id.0),
                lead: Point { x: 0.0, y: 0.0 },
                auto: rel(self.tree.cards[&v.id.0]),
            })
            .chain(kids.iter().map(|(k, auto)| SettleItem {
                key: self.groups[k.group].path.clone(),
                size: k.size,
                lead: k.lead,
                auto: rel(*auto),
            }))
            .collect();
        let local = layout::settle(&items, &|key| self.placement.at(key), FREE_GAP);
        let rect = if items.is_empty() {
            Rect {
                at: Point { x: 0.0, y: 0.0 },
                size: self.nest[i].empty,
            }
        } else {
            let contents: Vec<Rect> = items
                .iter()
                .map(|it| Rect {
                    at: local[&it.key],
                    size: it.size,
                })
                .collect();
            cover(&contents, self.inset(&self.groups[i].path))
        };
        Laid {
            group: i,
            members: self.members[i].iter().map(|v| v.id.0).collect(),
            local,
            lead: rect.at,
            size: rect.size,
            kids: kids.into_iter().map(|(k, _)| k).collect(),
        }
    }

    fn flatten(&self, g: &Laid, origin: Point, out: &mut Scene) {
        out.groups.push(Frame {
            label: self.groups[g.group].label.clone(),
            rect: Rect {
                at: origin,
                size: g.size,
            },
        });
        for &id in &g.members {
            let p = g.local[&id.to_string()];
            out.cards.push(Card {
                node: id,
                rect: Rect {
                    at: Point {
                        x: origin.x + p.x - g.lead.x,
                        y: origin.y + p.y - g.lead.y,
                    },
                    size: (self.size_of)(id),
                },
                inset: self.inset(&self.groups[g.group].path),
            });
        }
        for k in &g.kids {
            let p = g.local[&self.groups[k.group].path];
            let at = Point {
                x: origin.x + p.x - g.lead.x,
                y: origin.y + p.y - g.lead.y,
            };
            self.flatten(k, at, out);
        }
    }
}

struct Built<'a> {
    repo: &'a RepoView,
    key: String,
    level: Level<'a>,
    tops: Vec<Laid>,
    at: BTreeMap<String, Point>,
    lead: Point,
    size: Size,
}

fn cover(contents: &[Rect], inset: Inset) -> Rect {
    let x = contents
        .iter()
        .map(|b| b.at.x)
        .fold(f64::INFINITY, f64::min)
        - inset.side;
    let y = contents
        .iter()
        .map(|b| b.at.y)
        .fold(f64::INFINITY, f64::min)
        - inset.top;
    let right = contents
        .iter()
        .map(|b| b.at.x + b.size.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let bottom = contents
        .iter()
        .map(|b| b.at.y + b.size.height)
        .fold(f64::NEG_INFINITY, f64::max);
    Rect {
        at: Point { x, y },
        size: Size {
            width: right + inset.side - x,
            height: bottom + inset.side - y,
        },
    }
}

pub fn build(snap: &Snapshot, resized: &HashMap<u32, Size>) -> Scene {
    let spots: HashMap<String, Point> = snap
        .layout
        .placed
        .iter()
        .map(|placed| {
            let key = match &placed.spot {
                Spot::Repo { repo } => format!("repo:{}", repo.0),
                Spot::Worktree { path, .. } => path.clone(),
                Spot::Node { node } => node.0.to_string(),
            };
            (
                key,
                Point {
                    x: placed.at.x,
                    y: placed.at.y,
                },
            )
        })
        .collect();
    let arranged: HashSet<u32> = snap.layout.arranged.iter().map(|r| r.0).collect();
    let mut sizes: HashMap<u32, Size> = snap
        .sizes
        .iter()
        .map(|s| {
            (
                s.node.0,
                Size {
                    width: s.width,
                    height: s.height,
                },
            )
        })
        .collect();
    sizes.extend(resized);
    let simple: HashSet<u32> = snap
        .nodes
        .iter()
        .filter(|n| n.card_view == Some(CardView::Simple))
        .map(|n| n.id.0)
        .collect();
    let size_of = |id: NodeId| {
        if simple.contains(&id) {
            SIMPLE
        } else {
            sizes.get(&id).copied().unwrap_or(STANDARD)
        }
    };
    let vertical = matches!(DIRECTION, Direction::Down | Direction::Up);
    let reversed = matches!(DIRECTION, Direction::Up | Direction::Left);
    let rank = |key: &str| {
        spots
            .get(key)
            .map_or(f64::INFINITY, |p| if vertical { p.x } else { p.y })
    };
    let mut views: Vec<&NodeView> = snap.nodes.iter().collect();
    views.sort_by(|a, b| {
        rank(&a.id.to_string())
            .total_cmp(&rank(&b.id.to_string()))
            .then(a.id.0.cmp(&b.id.0))
    });
    let by_id: HashMap<u32, &NodeView> = views.iter().map(|v| (v.id.0, *v)).collect();
    let parent_of = |id: NodeId| by_id.get(&id).and_then(|v| v.parent.map(|p| p.0));
    let framed = snap.repos.len() > 1;
    let mut built: Vec<Built> = Vec::new();
    for repo in &snap.repos {
        let repo_views: Vec<&NodeView> = views
            .iter()
            .copied()
            .filter(|v| v.place.repo == repo.id)
            .collect();
        let (boxes, chips) = split_chips(groups_of(repo, &repo_views), &repo_views);
        let mut groups = boxes;
        groups.sort_by(|a, b| rank(&a.path).total_cmp(&rank(&b.path)));
        let mut chip_hosts = HashSet::new();
        if let Some(host) = groups.iter().find(|g| g.kind != GroupKind::Linked)
            && !chips.is_empty()
        {
            chip_hosts.insert(host.path.clone());
        }
        let parents = group_parents(&groups, &repo_views);
        let cards = attach_cards(&groups, &repo_views, &parents);
        let members: Vec<Vec<&NodeView>> = groups
            .iter()
            .map(|g| {
                repo_views
                    .iter()
                    .copied()
                    .filter(|v| v.place.worktree == g.path)
                    .collect()
            })
            .collect();
        let nest: Vec<NestGroup> = (0..groups.len())
            .map(|i| NestGroup {
                parent: cards[i].and(parents[i]),
                card: cards[i],
                members: members[i].iter().map(|v| v.id.0).collect(),
                empty: EMPTY_GROUP,
            })
            .collect();
        let tree = layout::arrange_nested(
            &nest,
            &parent_of,
            &size_of,
            DIRECTION,
            Inset {
                top: CARD_TOP,
                side: CARD_X,
            },
            &|kid| match kid {
                Kid::Card(id) => rank(&id.to_string()),
                Kid::Group(g) => rank(&groups[g].path),
            },
        );
        let level = Level {
            groups,
            members,
            nest,
            tree,
            chips: chip_hosts,
            size_of: &size_of,
            placement: Placement {
                spots: &spots,
                arranged: arranged.contains(&repo.id.0),
            },
        };
        let tops: Vec<(Laid, Point)> = (0..level.nest.len())
            .filter(|&i| level.nest[i].parent.is_none())
            .map(|i| (level.lay(i), level.tree.boxes[i].at))
            .collect();
        if tops.is_empty() {
            continue;
        }
        let items: Vec<SettleItem> = tops
            .iter()
            .map(|(g, auto)| SettleItem {
                key: level.groups[g.group].path.clone(),
                size: g.size,
                lead: g.lead,
                auto: *auto,
            })
            .collect();
        let at = layout::settle(&items, &|key| level.placement.at(key), FREE_GAP);
        let contents: Vec<Rect> = tops
            .iter()
            .map(|(g, _)| Rect {
                at: at[&level.groups[g.group].path],
                size: g.size,
            })
            .collect();
        let rect = cover(&contents, REPO_INSET);
        built.push(Built {
            repo,
            key: format!("repo:{}", repo.id.0),
            level,
            tops: tops.into_iter().map(|(g, _)| g).collect(),
            at,
            lead: rect.at,
            size: rect.size,
        });
    }
    let along = |s: Size| if vertical { s.height } else { s.width };
    let deepest = built.iter().map(|b| along(b.size)).fold(0.0, f64::max);
    let mut row: Vec<&Built> = built.iter().collect();
    row.sort_by(|a, b| rank(&a.key).total_cmp(&rank(&b.key)));
    let items: Vec<SettleItem> = row
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let across: f64 = row[..i]
                .iter()
                .map(|q| if vertical { q.size.width } else { q.size.height } + GROUP_GAP)
                .sum();
            let along = if reversed {
                deepest - along(r.size)
            } else {
                0.0
            };
            let (x, y) = if vertical {
                (across, along)
            } else {
                (along, across)
            };
            SettleItem {
                key: r.key.clone(),
                size: r.size,
                lead: r.lead,
                auto: Point {
                    x: GROUP_POSITION.x + x,
                    y: GROUP_POSITION.y + y,
                },
            }
        })
        .collect();
    let repo_at = layout::settle(&items, &|key| spots.get(key).copied(), FREE_GAP);
    let mut scene = Scene {
        direction: DIRECTION,
        ..Scene::default()
    };
    for b in &built {
        let origin = if framed {
            let at = repo_at[&b.key];
            scene.repos.push(Frame {
                label: b.repo.name.clone(),
                rect: Rect { at, size: b.size },
            });
            Point {
                x: at.x - b.lead.x,
                y: at.y - b.lead.y,
            }
        } else {
            GROUP_POSITION
        };
        for g in &b.tops {
            let at = b.at[&b.level.groups[g.group].path];
            b.level.flatten(
                g,
                Point {
                    x: origin.x + at.x,
                    y: origin.y + at.y,
                },
                &mut scene,
            );
        }
    }
    scene
}
