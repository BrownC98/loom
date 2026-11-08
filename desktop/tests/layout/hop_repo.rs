use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::hops::{
    HopEnd, HopEndName, HopLabel, HopPair, HopPart, LabelSizes, hop_parts, pick_hue, sibling_lanes,
};
use crate::layout::{Direction, Point, Rect, Size};
use crate::repos::{
    Group, GroupKind, attach_cards, bookmarked, chosen_repo, group_parents, groups_of, in_group,
    place_name, split_chips,
};
use api::graph::NodeView;
use api::ids::RepoId;
use api::repos::RepoView;

#[derive(Deserialize, Debug, PartialEq, Clone, Copy)]
pub struct P {
    x: f64,
    y: f64,
}

#[derive(Deserialize, Clone, Copy)]
pub struct S {
    width: f64,
    height: f64,
}

#[derive(Deserialize, Clone, Copy)]
pub struct B {
    at: P,
    size: S,
}

fn point(p: P) -> Point {
    Point { x: p.x, y: p.y }
}

fn size(s: S) -> Size {
    Size {
        width: s.width,
        height: s.height,
    }
}

fn rect(b: B) -> Rect {
    Rect {
        at: point(b.at),
        size: size(b.size),
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Dir {
    Down,
    Right,
    Up,
    Left,
}

fn direction(d: Dir) -> Direction {
    match d {
        Dir::Down => Direction::Down,
        Dir::Right => Direction::Right,
        Dir::Up => Direction::Up,
        Dir::Left => Direction::Left,
    }
}

#[derive(Deserialize)]
pub struct Counts {
    source: u32,
    target: u32,
}

#[derive(Deserialize)]
pub struct PartCase {
    counts: Counts,
    source: Option<P>,
    target: Option<P>,
    turned: bool,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum EndName {
    Source,
    Target,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct PartOut {
    from: EndName,
    to: EndName,
    back: bool,
}

#[derive(Deserialize)]
pub struct PartsIn {
    cases: Vec<PartCase>,
}

fn end_name(name: HopEndName) -> EndName {
    match name {
        HopEndName::Source => EndName::Source,
        HopEndName::Target => EndName::Target,
    }
}

fn part_out(part: HopPart) -> PartOut {
    PartOut {
        from: end_name(part.from),
        to: end_name(part.to),
        back: part.back,
    }
}

pub fn parts(i: &PartsIn) -> Vec<Vec<PartOut>> {
    i.cases
        .iter()
        .map(|c| {
            hop_parts(
                [c.counts.source, c.counts.target],
                c.source.map(point),
                c.target.map(point),
                c.turned,
            )
            .into_iter()
            .map(part_out)
            .collect()
        })
        .collect()
}

#[derive(Deserialize)]
pub struct HueIn {
    shown: Vec<f64>,
    randoms: Vec<f64>,
}

pub fn hue(i: &HueIn) -> f64 {
    let mut randoms = i.randoms.iter().copied();
    pick_hue(&i.shown, || randoms.next().expect("randoms"))
}

#[derive(Deserialize)]
pub struct EndIn {
    id: u32,
    #[serde(rename = "box")]
    rect: B,
    room: B,
    bend: Option<f64>,
}

#[derive(Deserialize)]
pub struct LabelIn {
    full: S,
    short: S,
}

#[derive(Deserialize)]
pub struct PairIn {
    key: String,
    source: EndIn,
    target: EndIn,
    label: LabelIn,
}

#[derive(Deserialize)]
pub struct LanesIn {
    parent: B,
    pairs: Vec<PairIn>,
    direction: Dir,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct LabelOut {
    at: P,
    short: bool,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct RouteOut {
    points: Vec<P>,
    label: Option<LabelOut>,
    turned: bool,
}

fn end(e: &EndIn) -> HopEnd {
    HopEnd {
        id: e.id,
        rect: rect(e.rect),
        room: rect(e.room),
        bend: e.bend,
    }
}

fn out_p(p: Point) -> P {
    P { x: p.x, y: p.y }
}

pub fn lanes(i: &LanesIn) -> BTreeMap<String, RouteOut> {
    let pairs: Vec<HopPair> = i
        .pairs
        .iter()
        .map(|p| HopPair {
            key: p.key.clone(),
            source: end(&p.source),
            target: end(&p.target),
            label: LabelSizes {
                full: size(p.label.full),
                short: size(p.label.short),
            },
        })
        .collect();
    sibling_lanes(rect(i.parent), &pairs, direction(i.direction))
        .into_iter()
        .map(|(key, route)| {
            let label = route.label.map(|HopLabel { at, short }| LabelOut {
                at: out_p(at),
                short,
            });
            (
                key,
                RouteOut {
                    points: route.points.into_iter().map(out_p).collect(),
                    label,
                    turned: route.turned,
                },
            )
        })
        .collect()
}

#[derive(Deserialize)]
pub struct WorktreeIn {
    path: String,
    branch: Option<String>,
    original: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoIn {
    id: u32,
    name: String,
    has_commits: bool,
    worktrees: Vec<WorktreeIn>,
}

#[derive(Deserialize)]
pub struct ViewIn {
    id: u32,
    parent: Option<u32>,
    repo: u32,
    worktree: String,
    bookmark: Option<u32>,
}

fn repo_view(r: &RepoIn) -> RepoView {
    let worktrees: Vec<Value> = r
        .worktrees
        .iter()
        .map(|w| json!({"path": w.path, "head": "", "branch": w.branch, "original": w.original}))
        .collect();
    serde_json::from_value(json!({
        "id": r.id,
        "name": r.name,
        "path": "",
        "worktrees": worktrees,
        "branches": [],
        "hasCommits": r.has_commits,
        "place": {"kind": "pc"},
        "shownPath": "",
    }))
    .expect("repo view")
}

fn node_view(v: &ViewIn) -> NodeView {
    serde_json::from_value(json!({
        "id": v.id,
        "name": "",
        "agent": null,
        "parent": v.parent,
        "state": {"kind": "running"},
        "process": {"kind": "working"},
        "unread": 0,
        "model": null,
        "effort": null,
        "place": {"repo": v.repo, "worktree": v.worktree},
        "bookmark": v.bookmark,
        "cardView": null,
        "reply": null,
        "waits": [],
        "relaunch": null,
        "status": {"kind": "idle"},
        "since": 0,
        "rework": 0,
    }))
    .expect("node view")
}

#[derive(Deserialize)]
pub struct GroupsIn {
    repo: RepoIn,
    views: Vec<ViewIn>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum KindOut {
    Original,
    Unborn,
    Linked,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct GroupOut {
    path: String,
    label: String,
    kind: KindOut,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct GroupsOut {
    groups: Vec<GroupOut>,
    boxes: Vec<String>,
    chips: Vec<String>,
    parents: Vec<Option<usize>>,
    attach: Vec<Option<u32>>,
    #[serde(rename = "inGroup")]
    in_group: Vec<Vec<u32>>,
}

fn group_out(g: &Group) -> GroupOut {
    GroupOut {
        path: g.path.clone(),
        label: g.label.clone(),
        kind: match g.kind {
            GroupKind::Original => KindOut::Original,
            GroupKind::Unborn => KindOut::Unborn,
            GroupKind::Linked => KindOut::Linked,
        },
    }
}

pub fn groups(i: &GroupsIn) -> GroupsOut {
    let repo = repo_view(&i.repo);
    let owned: Vec<NodeView> = i.views.iter().map(node_view).collect();
    let views: Vec<&NodeView> = owned.iter().collect();
    let groups = groups_of(&repo, &views);
    let parents = group_parents(&groups, &views);
    let attach = attach_cards(&groups, &views, &parents);
    let in_groups = groups
        .iter()
        .map(|g| in_group(&views, g).iter().map(|v| v.id.0).collect())
        .collect();
    let (boxes, chips) = split_chips(groups.clone(), &views);
    GroupsOut {
        groups: groups.iter().map(group_out).collect(),
        boxes: boxes.into_iter().map(|g| g.path).collect(),
        chips: chips.into_iter().map(|g| g.path).collect(),
        parents,
        attach,
        in_group: in_groups,
    }
}

#[derive(Deserialize)]
pub struct PlacesIn {
    repos: Vec<RepoIn>,
    views: Vec<ViewIn>,
    saved: Vec<Option<u32>>,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct PlacesOut {
    names: Vec<String>,
    bookmarked: Vec<u32>,
    chosen: Vec<Option<u32>>,
}

pub fn places(i: &PlacesIn) -> PlacesOut {
    let repos: Vec<RepoView> = i.repos.iter().map(repo_view).collect();
    let owned: Vec<NodeView> = i.views.iter().map(node_view).collect();
    let views: Vec<&NodeView> = owned.iter().collect();
    PlacesOut {
        names: views
            .iter()
            .map(|v| place_name(&repos, &views, v))
            .collect(),
        bookmarked: bookmarked(&views).iter().map(|v| v.id.0).collect(),
        chosen: i
            .saved
            .iter()
            .map(|s| chosen_repo(&repos, s.map(RepoId)).map(|r| r.id.0))
            .collect(),
    }
}
