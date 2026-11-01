use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use api::canvas::{self, Arranged, Placed, Spot};
use api::ids::RepoId;
use api::settings::{ArrangeDirection, SettingsView};

use super::layout::{self, Direction, Inset, Point, Rect, Sibling, Swap};
use super::scene::{FREE_GAP, REPO_INSET, Scene, cover};
use crate::link::{self, Snapshot};
use crate::problem::Failure;

pub struct Tier {
    pub boxes: Vec<Sibling>,
    pub lead: HashMap<String, Point>,
    pub parent: Option<String>,
    pub shell: Inset,
}

pub struct Overlay {
    pub direction: Direction,
    pub always: bool,
    pub moving: HashMap<String, Point>,
    pub drafts: HashMap<String, Point>,
    pub clearing: HashSet<u32>,
    pub order: HashMap<String, f64>,
    pub previous: HashMap<String, String>,
}

pub struct Arrange {
    pub direction: ArrangeDirection,
}

pub fn direction_of(direction: ArrangeDirection) -> Direction {
    match direction {
        ArrangeDirection::Down => Direction::Down,
        ArrangeDirection::Right => Direction::Right,
        ArrangeDirection::Up => Direction::Up,
        ArrangeDirection::Left => Direction::Left,
    }
}

enum Write {
    Clear(Vec<u32>),
    Spots(Vec<Placed>),
    Arranged(Arranged),
}

pub struct Job {
    id: u64,
    writes: Vec<Write>,
}

#[derive(Clone, Copy)]
pub enum Scope {
    All,
    Repo(u32),
}

impl Job {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn run(&self, base: &str) -> Result<(), Failure> {
        for write in &self.writes {
            match write {
                Write::Clear(repos) => {
                    for repo in repos {
                        link::clear_spots(base, *repo)?;
                    }
                }
                Write::Spots(placed) => link::put_spots(base, placed)?,
                Write::Arranged(arranged) => link::put_arranged(base, arranged)?,
            }
        }
        Ok(())
    }
}

struct Active {
    key: String,
    from: Point,
    swap: Swap,
    dragged: Rect,
}

struct Draft {
    id: u64,
    spots: HashMap<String, Point>,
    clearing: Vec<u32>,
    direction: Option<ArrangeDirection>,
    settled: Option<u64>,
}

#[derive(Default)]
pub struct Moves {
    active: Option<Active>,
    moving: HashMap<String, Point>,
    drafts: Vec<Draft>,
    order: HashMap<String, f64>,
    issued: u64,
}

struct Step {
    key: String,
    tier: Rc<Tier>,
    mover: Rect,
}

fn origin_of(tier: &Tier, key: &str, at: Point) -> Point {
    let lead = tier
        .lead
        .get(key)
        .copied()
        .unwrap_or(Point { x: 0.0, y: 0.0 });
    Point {
        x: at.x - lead.x,
        y: at.y - lead.y,
    }
}

fn pinned(tier: &Tier, prev: &HashMap<String, Point>, scene: &Scene) -> HashMap<String, Point> {
    let mut out = prev.clone();
    for b in &tier.boxes {
        out.entry(b.key.clone())
            .or_insert_with(|| origin_of(tier, &b.key, b.rect.at));
    }
    let mut parent = tier.parent.clone();
    while let Some(key) = parent {
        let Some(outer) = scene.tiers.get(&key) else {
            break;
        };
        if let Some(me) = outer.boxes.iter().find(|b| b.key == key) {
            out.entry(key.clone())
                .or_insert_with(|| origin_of(outer, &key, me.rect.at));
        }
        parent = outer.parent.clone();
    }
    out
}

fn placed(tier: &Tier, done: &HashMap<String, Point>) -> Vec<Sibling> {
    tier.boxes
        .iter()
        .map(|b| match done.get(&b.key) {
            Some(o) => {
                let lead = tier
                    .lead
                    .get(&b.key)
                    .copied()
                    .unwrap_or(Point { x: 0.0, y: 0.0 });
                Sibling {
                    key: b.key.clone(),
                    rect: Rect {
                        at: Point {
                            x: o.x + lead.x,
                            y: o.y + lead.y,
                        },
                        size: b.rect.size,
                    },
                }
            }
            None => b.clone(),
        })
        .collect()
}

fn push_overlaps(step: &Step, done: &mut HashMap<String, Point>) {
    let others: Vec<Sibling> = placed(&step.tier, done)
        .into_iter()
        .filter(|b| b.key != step.key)
        .collect();
    let targets: Vec<String> = others
        .iter()
        .filter(|o| layout::overlaps(o.rect, step.mover))
        .map(|o| o.key.clone())
        .collect();
    for (key, at) in layout::push(&others, step.mover, &targets, FREE_GAP) {
        let origin = origin_of(&step.tier, &key, at);
        done.insert(key, origin);
    }
}

fn shell_of(step: &Step, scene: &Scene, done: &HashMap<String, Point>) -> Option<Step> {
    let parent = step.tier.parent.as_ref()?;
    let outer = scene.tiers.get(parent)?.clone();
    let contents: Vec<Rect> = placed(&step.tier, done)
        .iter()
        .map(|b| {
            if b.key == step.key {
                step.mover
            } else {
                b.rect
            }
        })
        .collect();
    let shell = cover(&contents, step.tier.shell);
    let me = outer.boxes.iter().find(|b| b.key == *parent)?;
    let origin = done
        .get(parent)
        .copied()
        .unwrap_or_else(|| origin_of(&outer, parent, me.rect.at));
    Some(Step {
        key: parent.clone(),
        tier: outer,
        mover: Rect {
            at: Point {
                x: origin.x + shell.at.x,
                y: origin.y + shell.at.y,
            },
            size: shell.size,
        },
    })
}

fn spread(scene: &Scene, first: Step, done: &mut HashMap<String, Point>) {
    let mut step = Some(first);
    while let Some(current) = step {
        push_overlaps(&current, done);
        step = shell_of(&current, scene, done);
    }
}

fn cross(direction: Direction, p: Point) -> f64 {
    if matches!(direction, Direction::Down | Direction::Up) {
        p.x
    } else {
        p.y
    }
}

fn repo_of(snap: &Snapshot, spot: &Spot) -> Option<u32> {
    match spot {
        Spot::Repo { .. } => None,
        Spot::Worktree { repo, .. } => Some(repo.0),
        Spot::Node { node } => snap
            .nodes
            .iter()
            .find(|n| n.id == *node)
            .map(|n| n.place.repo.0),
    }
}

fn keep_around(scene: &Scene, key: &str, new_lead: Point) -> HashMap<String, Point> {
    let Some(tier) = scene.tiers.get(key) else {
        return HashMap::new();
    };
    let mut out = pinned(tier, &HashMap::new(), scene);
    if let Some(me) = tier.boxes.iter().find(|b| b.key == key) {
        out.insert(
            key.to_owned(),
            Point {
                x: me.rect.at.x - new_lead.x,
                y: me.rect.at.y - new_lead.y,
            },
        );
    }
    out
}

fn write_of(scene: &Scene, snap: &Snapshot, done: &HashMap<String, Point>, keep: bool) -> Write {
    let placed = place(scene, done);
    if !keep {
        return Write::Spots(placed);
    }
    let repos: HashSet<u32> = placed
        .iter()
        .filter_map(|p| repo_of(snap, &p.spot))
        .collect();
    Write::Arranged(Arranged {
        repos: repos.into_iter().map(RepoId).collect(),
        placed,
    })
}

fn place(scene: &Scene, spots: &HashMap<String, Point>) -> Vec<Placed> {
    spots
        .iter()
        .filter_map(|(key, p)| {
            Some(Placed {
                spot: scene.spots.get(key)?.clone(),
                at: canvas::Point { x: p.x, y: p.y },
            })
        })
        .collect()
}

impl Moves {
    pub fn dragging(&self) -> bool {
        self.active.is_some()
    }

    pub fn grabbed(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.key.as_str())
    }

    pub fn overlay(&self, settings: &SettingsView, previous: HashMap<String, String>) -> Overlay {
        let mut drafts = HashMap::new();
        let mut clearing = HashSet::new();
        for d in &self.drafts {
            drafts.extend(d.spots.iter().map(|(k, p)| (k.clone(), *p)));
            clearing.extend(d.clearing.iter().copied());
        }
        let wanted = self
            .drafts
            .iter()
            .rev()
            .find_map(|d| d.direction)
            .unwrap_or(settings.arrange_direction);
        Overlay {
            direction: direction_of(wanted),
            always: settings.always_arrange,
            moving: self.moving.clone(),
            drafts,
            clearing,
            order: self.order.clone(),
            previous,
        }
    }

    fn hold(&mut self, scene: &Scene, direction: Direction) {
        self.order.extend(
            scene
                .cards
                .iter()
                .map(|c| (c.node.to_string(), cross(direction, c.rect.at)))
                .chain(
                    scene
                        .repos
                        .iter()
                        .chain(&scene.groups)
                        .map(|g| (g.key.clone(), cross(direction, g.rect.at))),
                ),
        );
    }

    pub fn begin(&mut self, key: &str, scene: &Scene, settings: &SettingsView) -> bool {
        let Some(b) = scene
            .tiers
            .get(key)
            .and_then(|t| t.boxes.iter().find(|b| b.key == key))
        else {
            return false;
        };
        self.moving.clear();
        self.hold(scene, direction_of(settings.arrange_direction));
        self.active = Some(Active {
            key: key.to_owned(),
            from: b.rect.at,
            swap: Swap {
                slot: b.rect,
                sides: BTreeMap::new(),
                at: BTreeMap::new(),
            },
            dragged: b.rect,
        });
        true
    }

    pub fn drag(&mut self, scene: &Scene, delta: Point, always: bool) -> bool {
        let Some(active) = &mut self.active else {
            return false;
        };
        let Some(tier) = scene.tiers.get(&active.key) else {
            return false;
        };
        let Some(me) = tier.boxes.iter().find(|b| b.key == active.key) else {
            return false;
        };
        let dragged = Rect {
            at: Point {
                x: active.from.x + delta.x,
                y: active.from.y + delta.y,
            },
            size: me.rect.size,
        };
        let branch = scene.branches.get(&active.key);
        let others: Vec<Sibling> = tier
            .boxes
            .iter()
            .filter(|b| b.key != active.key && (!always || scene.branches.get(&b.key) == branch))
            .cloned()
            .collect();
        let next = layout::swap(&active.swap, &others, dragged);
        let mut moving = pinned(tier, &self.moving, scene);
        moving.insert(active.key.clone(), origin_of(tier, &active.key, dragged.at));
        for (key, at) in &next.at {
            moving.insert(key.clone(), origin_of(tier, key, *at));
        }
        active.swap = next;
        active.dragged = dragged;
        self.moving = moving;
        true
    }

    pub fn cancel(&mut self) {
        self.active = None;
        self.moving.clear();
    }

    pub fn drop(&mut self, scene: &Scene, snap: &Snapshot, always: bool) -> Option<Job> {
        let active = self.active.take()?;
        let moved = std::mem::take(&mut self.moving);
        if moved.is_empty() {
            return None;
        }
        let tier = scene.tiers.get(&active.key)?.clone();
        let mut done = moved;
        done.insert(
            active.key.clone(),
            origin_of(&tier, &active.key, active.dragged.at),
        );
        spread(
            scene,
            Step {
                key: active.key,
                tier,
                mover: active.dragged,
            },
            &mut done,
        );
        self.save(scene, snap, done, always)
    }

    fn save(
        &mut self,
        scene: &Scene,
        snap: &Snapshot,
        done: HashMap<String, Point>,
        keep: bool,
    ) -> Option<Job> {
        let held: HashMap<&String, &Point> = self.drafts.iter().flat_map(|d| &d.spots).collect();
        let done: HashMap<String, Point> = done
            .into_iter()
            .filter(|(k, p)| held.get(k).is_none_or(|d| !layout::same_point(**d, *p)))
            .collect();
        if done.is_empty() {
            return None;
        }
        let writes = vec![write_of(scene, snap, &done, keep)];
        Some(self.commit(done, Vec::new(), None, writes))
    }

    fn commit(
        &mut self,
        spots: HashMap<String, Point>,
        clearing: Vec<u32>,
        direction: Option<ArrangeDirection>,
        writes: Vec<Write>,
    ) -> Job {
        self.issued += 1;
        self.drafts.push(Draft {
            id: self.issued,
            spots,
            clearing,
            direction,
            settled: None,
        });
        Job {
            id: self.issued,
            writes,
        }
    }

    pub fn arranging(
        &mut self,
        scene: &Scene,
        settings: &SettingsView,
        direction: ArrangeDirection,
        scope: Scope,
    ) -> Overlay {
        self.moving.clear();
        self.active = None;
        self.hold(scene, direction_of(settings.arrange_direction));
        let mut overlay = self.overlay(settings, scene.outers.clone());
        overlay.direction = direction_of(direction);
        overlay.clearing = match scope {
            Scope::All => scene.autos.keys().copied().collect(),
            Scope::Repo(id) => HashSet::from([id]),
        };
        overlay
    }

    pub fn arranged(
        &mut self,
        scene: &Scene,
        cleared: &Scene,
        snap: &Snapshot,
        direction: ArrangeDirection,
        scope: Scope,
        always: bool,
    ) -> Job {
        let mut ids: Vec<u32> = match scope {
            Scope::All => cleared.autos.keys().copied().collect(),
            Scope::Repo(id) => vec![id],
        };
        ids.sort_unstable();
        let mut spots: HashMap<String, Point> = HashMap::new();
        let mut writes = vec![Write::Clear(ids.clone())];
        if let Scope::Repo(id) = scope {
            let key = format!("repo:{id}");
            let kept = keep_around(
                scene,
                &key,
                Point {
                    x: -REPO_INSET.side,
                    y: -REPO_INSET.top,
                },
            );
            writes.push(write_of(scene, snap, &kept, always));
            spots.extend(kept);
        }
        for id in &ids {
            if let Some(auto) = cleared.autos.get(id) {
                spots.extend(auto.iter().map(|(k, p)| (k.clone(), *p)));
            }
        }
        if matches!(scope, Scope::All) {
            spots.extend(cleared.repo_autos.iter().map(|(k, p)| (k.clone(), *p)));
        }
        let arranged: HashMap<String, Point> = spots
            .iter()
            .filter(|(k, _)| match scope {
                Scope::All => true,
                Scope::Repo(id) => cleared
                    .autos
                    .get(&id)
                    .is_some_and(|auto| auto.contains_key(*k)),
            })
            .map(|(k, p)| (k.clone(), *p))
            .collect();
        writes.push(Write::Arranged(Arranged {
            repos: ids.iter().copied().map(RepoId).collect(),
            placed: place(cleared, &arranged),
        }));
        self.commit(spots, ids, Some(direction), writes)
    }

    pub fn arrange_group(
        &mut self,
        scene: &Scene,
        snap: &Snapshot,
        settings: &SettingsView,
        path: &str,
    ) -> Option<Job> {
        self.hold(scene, direction_of(settings.arrange_direction));
        let auto = scene.group_autos.get(path)?;
        let repo = match scene.spots.get(path)? {
            Spot::Worktree { repo, .. } => repo.0,
            _ => return None,
        };
        let mut done = keep_around(scene, path, Point { x: 0.0, y: 0.0 });
        done.extend(auto.iter().map(|(k, p)| (k.clone(), *p)));
        let keep = settings.always_arrange
            || (snap.layout.arranged.contains(&RepoId(repo))
                && !self.drafts.iter().any(|d| d.clearing.contains(&repo)));
        self.save(scene, snap, done, keep)
    }

    pub fn finish(&mut self, id: u64) {
        let mark = link::issued();
        if let Some(d) = self.drafts.iter_mut().find(|d| d.id == id) {
            d.settled = Some(mark);
        }
    }

    pub fn retire(&mut self, loaded: u64) -> bool {
        let before = self.drafts.len();
        self.drafts
            .retain(|d| d.settled.is_none_or(|mark| loaded <= mark));
        self.drafts.len() != before
    }
}
