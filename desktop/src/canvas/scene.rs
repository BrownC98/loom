use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use api::canvas::Spot;
use api::ids::RepoId;

use super::drag::{Overlay, Tier};
use super::layout::{
    self, Direction, GROUP_GAP, Inset, Kid, NestGroup, Nested, NodeId, Point, Rect, SettleItem,
    Sibling, Size,
};
use super::repos::{Group, GroupKind, attach_cards, group_parents, groups_of, split_chips};
use api::graph::{CardView, NodeView, Place};
use api::repos::RepoView;
use api::settings::DefaultCardView;

use crate::link::Snapshot;
use crate::views::card;

const CARD_X: f64 = 20.0;
const CARD_TOP: f64 = 60.0;
const CHIP_ROW: f64 = 43.0;
const EMPTY_GROUP: Size = Size {
    width: 320.0,
    height: 100.0,
};
pub const FREE_GAP: f64 = 32.0;
pub const REPO_INSET: Inset = Inset {
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
    pub key: String,
    pub label: String,
    pub rect: Rect,
    pub place: Option<Place>,
}

pub struct Card {
    pub node: NodeId,
    pub rect: Rect,
    pub inset: Inset,
    pub spot: Point,
}

#[derive(Default)]
pub struct Scene {
    pub direction: Direction,
    pub repos: Vec<Frame>,
    pub groups: Vec<Frame>,
    pub cards: Vec<Card>,
    pub tiers: HashMap<String, Rc<Tier>>,
    pub spots: HashMap<String, Spot>,
    pub branches: HashMap<String, String>,
    pub outers: HashMap<String, String>,
    pub group_autos: HashMap<String, BTreeMap<String, Point>>,
    pub autos: HashMap<u32, BTreeMap<String, Point>>,
    pub repo_autos: BTreeMap<String, Point>,
    pub chips: HashMap<String, String>,
}

struct Laid {
    group: usize,
    members: Vec<NodeId>,
    local: BTreeMap<String, Point>,
    auto: BTreeMap<String, Point>,
    lead: Point,
    size: Size,
    kids: Vec<Laid>,
    tier: Rc<Tier>,
}

struct Placement<'a> {
    overlay: &'a Overlay,
    spots: &'a HashMap<String, Point>,
    keep: bool,
    stale: HashSet<String>,
}

impl Placement<'_> {
    fn at(&self, key: &str) -> Option<Point> {
        if let Some(moving) = self.overlay.moving.get(key) {
            return Some(*moving);
        }
        if self.keep || self.stale.contains(key) {
            None
        } else {
            self.spots.get(key).copied()
        }
    }
}

struct Level<'a> {
    repo: RepoId,
    groups: Vec<Group>,
    members: Vec<Vec<&'a NodeView>>,
    nest: Vec<NestGroup>,
    tree: Nested,
    chips: HashSet<String>,
    size_of: &'a dyn Fn(NodeId) -> Size,
    placement: Placement<'a>,
}

fn unpiled(
    at: BTreeMap<String, Point>,
    items: &[SettleItem],
    overlay: &Overlay,
) -> BTreeMap<String, Point> {
    if !overlay.moving.is_empty() {
        return at;
    }
    let siblings: Vec<Sibling> = items
        .iter()
        .map(|it| Sibling {
            key: it.key.clone(),
            rect: Rect {
                at: at[&it.key],
                size: it.size,
            },
        })
        .collect();
    layout::unpile(&siblings, FREE_GAP)
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
        let local = unpiled(
            layout::settle(&items, &|key| self.placement.at(key), FREE_GAP),
            &items,
            self.placement.overlay,
        );
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
        let tier = Rc::new(Tier {
            boxes: items
                .iter()
                .map(|it| Sibling {
                    key: it.key.clone(),
                    rect: Rect {
                        at: local[&it.key],
                        size: it.size,
                    },
                })
                .collect(),
            lead: kids
                .iter()
                .map(|(k, _)| (self.groups[k.group].path.clone(), k.lead))
                .collect(),
            parent: Some(self.groups[i].path.clone()),
            shell: self.inset(&self.groups[i].path),
        });
        let auto = items
            .iter()
            .map(|it| {
                (
                    it.key.clone(),
                    Point {
                        x: it.auto.x - it.lead.x,
                        y: it.auto.y - it.lead.y,
                    },
                )
            })
            .collect();
        Laid {
            group: i,
            members: self.members[i].iter().map(|v| v.id.0).collect(),
            local,
            auto,
            tier,
            lead: rect.at,
            size: rect.size,
            kids: kids.into_iter().map(|(k, _)| k).collect(),
        }
    }

    fn flatten(&self, g: &Laid, origin: Point, out: &mut Scene) {
        let path = &self.groups[g.group].path;
        for b in &g.tier.boxes {
            out.tiers.insert(b.key.clone(), g.tier.clone());
        }
        out.group_autos.insert(path.clone(), g.auto.clone());
        for v in &self.members[g.group] {
            out.spots
                .insert(v.id.to_string(), Spot::Node { node: v.id });
            let branch = v
                .parent
                .filter(|p| self.members[g.group].iter().any(|m| m.id == *p))
                .map_or_else(|| "root".to_owned(), |p| p.to_string());
            out.branches.insert(v.id.to_string(), branch);
        }
        for k in &g.kids {
            let kid = &self.groups[k.group].path;
            out.spots.insert(
                kid.clone(),
                Spot::Worktree {
                    repo: self.repo,
                    path: kid.clone(),
                },
            );
            let card = self.nest[k.group]
                .card
                .map_or_else(String::new, |c| c.to_string());
            out.branches.insert(kid.clone(), card);
        }
        out.groups.push(Frame {
            key: path.clone(),
            label: self.groups[g.group].label.clone(),
            rect: Rect {
                at: origin,
                size: g.size,
            },
            place: Some(Place {
                repo: self.repo,
                worktree: self.groups[g.group].path.clone(),
            }),
        });
        for &id in &g.members {
            let p = g.local[&id.to_string()];
            out.cards.push(Card {
                node: id,
                spot: p,
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
    auto: BTreeMap<String, Point>,
}

fn all_autos(g: &Laid, out: &mut BTreeMap<String, Point>) {
    out.extend(g.auto.iter().map(|(k, p)| (k.clone(), *p)));
    for kid in &g.kids {
        all_autos(kid, out);
    }
}

pub fn cover(contents: &[Rect], inset: Inset) -> Rect {
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

pub fn build(
    snap: &Snapshot,
    shown: &[RepoId],
    card_view: DefaultCardView,
    resized: &HashMap<u32, Size>,
    overlay: &Overlay,
) -> Scene {
    let repo_of = |spot: &Spot| match spot {
        Spot::Repo { repo } | Spot::Worktree { repo, .. } => Some(repo.0),
        Spot::Node { node } => snap
            .nodes
            .iter()
            .find(|n| n.id == *node)
            .map(|n| n.place.repo.0),
    };
    let mut spots: HashMap<String, Point> = snap
        .layout
        .placed
        .iter()
        .filter(|placed| repo_of(&placed.spot).is_none_or(|r| !overlay.clearing.contains(&r)))
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
    spots.extend(overlay.drafts.iter().map(|(k, p)| (k.clone(), *p)));
    let arranged: HashSet<u32> = snap
        .layout
        .arranged
        .iter()
        .map(|r| r.0)
        .filter(|r| !overlay.clearing.contains(r))
        .collect();
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
        .filter(|n| card::view_of(n, card_view) == CardView::Simple)
        .map(|n| n.id.0)
        .collect();
    let size_of = |id: NodeId| {
        if simple.contains(&id) {
            SIMPLE
        } else {
            sizes.get(&id).copied().unwrap_or(STANDARD)
        }
    };
    let direction = overlay.direction;
    let vertical = matches!(direction, Direction::Down | Direction::Up);
    let reversed = matches!(direction, Direction::Up | Direction::Left);
    let rank = |key: &str| {
        spots.get(key).map_or_else(
            || overlay.order.get(key).copied().unwrap_or(f64::INFINITY),
            |p| if vertical { p.x } else { p.y },
        )
    };
    let mut views: Vec<&NodeView> = snap.nodes.iter().collect();
    views.sort_by(|a, b| {
        rank(&a.id.to_string())
            .total_cmp(&rank(&b.id.to_string()))
            .then(a.id.0.cmp(&b.id.0))
    });
    let by_id: HashMap<u32, &NodeView> = views.iter().map(|v| (v.id.0, *v)).collect();
    let parent_of = |id: NodeId| by_id.get(&id).and_then(|v| v.parent.map(|p| p.0));
    let framed = shown.len() > 1;
    let mut built: Vec<Built> = Vec::new();
    let mut scene = Scene {
        direction,
        ..Scene::default()
    };
    for repo in snap.repos.iter().filter(|r| shown.contains(&r.id)) {
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
            scene
                .chips
                .extend(chips.iter().map(|c| (c.path.clone(), host.path.clone())));
        }
        let parents = group_parents(&groups, &repo_views);
        let cards = attach_cards(&groups, &repo_views, &parents);
        let repo_key = format!("repo:{}", repo.id.0);
        let mut stale = HashSet::new();
        for (i, g) in groups.iter().enumerate() {
            let outer = match cards[i].and(parents[i]) {
                Some(p) => groups[p].path.clone(),
                None => repo_key.clone(),
            };
            if overlay
                .previous
                .get(&g.path)
                .is_some_and(|before| *before != outer)
            {
                stale.insert(g.path.clone());
            }
            scene.outers.insert(g.path.clone(), outer);
        }
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
            direction,
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
            repo: repo.id,
            groups,
            members,
            nest,
            tree,
            chips: chip_hosts,
            size_of: &size_of,
            placement: Placement {
                overlay,
                spots: &spots,
                keep: overlay.always || arranged.contains(&repo.id.0),
                stale,
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
        let at = unpiled(
            layout::settle(&items, &|key| level.placement.at(key), FREE_GAP),
            &items,
            overlay,
        );
        let contents: Vec<Rect> = tops
            .iter()
            .map(|(g, _)| Rect {
                at: at[&level.groups[g.group].path],
                size: g.size,
            })
            .collect();
        let rect = cover(&contents, REPO_INSET);
        let top_tier = Rc::new(Tier {
            boxes: tops
                .iter()
                .map(|(g, _)| Sibling {
                    key: level.groups[g.group].path.clone(),
                    rect: Rect {
                        at: at[&level.groups[g.group].path],
                        size: g.size,
                    },
                })
                .collect(),
            lead: tops
                .iter()
                .map(|(g, _)| (level.groups[g.group].path.clone(), g.lead))
                .collect(),
            parent: framed.then(|| repo_key.clone()),
            shell: REPO_INSET,
        });
        let mut auto = BTreeMap::new();
        for (g, top_auto) in &tops {
            all_autos(g, &mut auto);
            auto.insert(
                level.groups[g.group].path.clone(),
                Point {
                    x: top_auto.x - g.lead.x,
                    y: top_auto.y - g.lead.y,
                },
            );
            scene
                .tiers
                .insert(level.groups[g.group].path.clone(), top_tier.clone());
            scene.spots.insert(
                level.groups[g.group].path.clone(),
                Spot::Worktree {
                    repo: repo.id,
                    path: level.groups[g.group].path.clone(),
                },
            );
        }
        built.push(Built {
            repo,
            key: repo_key,
            level,
            tops: tops.into_iter().map(|(g, _)| g).collect(),
            at,
            lead: rect.at,
            size: rect.size,
            auto,
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
    let repo_placed = |key: &str| {
        overlay
            .moving
            .get(key)
            .copied()
            .or_else(|| spots.get(key).copied().filter(|_| !overlay.always))
    };
    let repo_at = unpiled(
        layout::settle(&items, &repo_placed, FREE_GAP),
        &items,
        overlay,
    );
    if framed {
        let tier = Rc::new(Tier {
            boxes: built
                .iter()
                .map(|b| Sibling {
                    key: b.key.clone(),
                    rect: Rect {
                        at: repo_at[&b.key],
                        size: b.size,
                    },
                })
                .collect(),
            lead: built.iter().map(|b| (b.key.clone(), b.lead)).collect(),
            parent: None,
            shell: REPO_INSET,
        });
        for b in &built {
            scene.tiers.insert(b.key.clone(), tier.clone());
        }
        scene.repo_autos = items
            .iter()
            .map(|it| {
                (
                    it.key.clone(),
                    Point {
                        x: it.auto.x - it.lead.x,
                        y: it.auto.y - it.lead.y,
                    },
                )
            })
            .collect();
    }
    for b in &built {
        scene
            .spots
            .insert(b.key.clone(), Spot::Repo { repo: b.repo.id });
        scene.autos.insert(b.repo.id.0, b.auto.clone());
        let origin = if framed {
            let at = repo_at[&b.key];
            scene.repos.push(Frame {
                key: b.key.clone(),
                label: b.repo.name.clone(),
                rect: Rect { at, size: b.size },
                place: None,
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
