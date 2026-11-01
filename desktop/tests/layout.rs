#[path = "../src/canvas/hops.rs"]
mod hops;
#[path = "../src/canvas/layout.rs"]
mod layout;
#[path = "../src/canvas/repos.rs"]
mod repos;

#[path = "../src/views/dock/lanes.rs"]
mod lanes;

#[path = "layout/commit_lanes.rs"]
mod commit_lanes;
#[path = "layout/hop_repo.rs"]
mod hop_repo;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use layout::{
    Camera, CenterSide, Direction, EdgeEnd, Inset, Kid, NestGroup, Point, Rect, SettleItem,
    Sibling, Side, Size, Swap,
};
use serde::Deserialize;

#[derive(Deserialize, Debug, PartialEq)]
struct P {
    x: f64,
    y: f64,
}

#[derive(Deserialize, Debug, PartialEq)]
struct S {
    width: f64,
    height: f64,
}

#[derive(Deserialize, Debug, PartialEq)]
struct R {
    at: P,
    size: S,
}

#[derive(Deserialize)]
struct G {
    parent: Option<usize>,
    card: Option<u32>,
    members: Vec<u32>,
    empty: S,
}

#[derive(Deserialize)]
struct InsetIn {
    top: f64,
    side: f64,
}

#[derive(Deserialize)]
struct NestedIn {
    direction: Dir,
    inset: InsetIn,
    groups: Vec<G>,
    parents: HashMap<String, Option<u32>>,
    sizes: HashMap<String, S>,
    rank: HashMap<String, f64>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct NestedOut {
    cards: BTreeMap<String, P>,
    boxes: Vec<R>,
}

#[derive(Deserialize)]
struct ItemIn {
    key: String,
    size: S,
    lead: P,
    auto: P,
}

#[derive(Deserialize)]
struct SettleIn {
    gap: f64,
    items: Vec<ItemIn>,
    placed: HashMap<String, P>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Sib {
    key: String,
    at: P,
    size: S,
}

#[derive(Deserialize)]
struct PushIn {
    siblings: Vec<Sib>,
    mover: R,
    targets: Vec<String>,
    gap: f64,
}

#[derive(Deserialize)]
struct UnpileIn {
    siblings: Vec<Sib>,
    gap: f64,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Copy)]
struct CS {
    x: f64,
    y: f64,
}

#[derive(Deserialize)]
struct SwapState {
    slot: R,
    sides: BTreeMap<String, CS>,
    at: BTreeMap<String, P>,
}

#[derive(Deserialize)]
struct SwapIn {
    state: SwapState,
    others: Vec<Sib>,
    dragged: R,
}

#[derive(Deserialize, Debug, PartialEq)]
struct SwapOut {
    slot: R,
    sides: BTreeMap<String, CS>,
    at: BTreeMap<String, P>,
}

#[derive(Deserialize)]
struct CenterSideIn {
    dragged: R,
    #[serde(rename = "box")]
    rect: R,
}

#[derive(Deserialize)]
struct CrossedIn {
    before: CS,
    dragged: R,
    #[serde(rename = "box")]
    rect: R,
}

#[derive(Deserialize)]
struct FreeSpotIn {
    #[serde(rename = "box")]
    rect: R,
    others: Vec<R>,
    gap: f64,
}

#[derive(Deserialize)]
struct SamePointIn {
    a: P,
    b: P,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
enum Dir {
    Down,
    Right,
    Up,
    Left,
}

#[derive(Deserialize)]
struct StepIn {
    parent: R,
    child: R,
    direction: Dir,
    bend: f64,
}

#[derive(Deserialize, Debug, PartialEq)]
struct StepOut {
    step: Option<f64>,
}

#[derive(Deserialize)]
struct BendIn {
    parent: R,
    bend: f64,
    direction: Dir,
}

#[derive(Deserialize, Debug, PartialEq)]
struct BendOut {
    x: Option<f64>,
    y: Option<f64>,
}

#[derive(Deserialize)]
struct EdgeIn {
    parent: R,
    child: R,
    direction: Dir,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum Sd {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Deserialize, Debug, PartialEq)]
struct EdgeOut {
    parent: Sd,
    child: Sd,
}

#[derive(Deserialize)]
struct EndIn {
    #[serde(rename = "box")]
    rect: R,
    side: Sd,
}

#[derive(Deserialize)]
struct HeadingIn {
    from: EndIn,
    to: EndIn,
    step: Option<f64>,
}

#[derive(Deserialize)]
struct HeadingOut {
    heading: Dir,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Copy)]
struct Cam {
    x: f64,
    y: f64,
    zoom: f64,
}

#[derive(Deserialize)]
struct FollowIn {
    old: P,
    now: P,
    camera: Cam,
}

#[derive(Deserialize)]
#[serde(tag = "kind")]
enum Case {
    #[serde(rename = "arrangeNested")]
    ArrangeNested { input: NestedIn, output: NestedOut },
    #[serde(rename = "settle")]
    Settle {
        input: SettleIn,
        output: BTreeMap<String, P>,
    },
    #[serde(rename = "hopParts")]
    HopParts {
        input: hop_repo::PartsIn,
        output: Vec<Vec<hop_repo::PartOut>>,
    },
    #[serde(rename = "pickHue")]
    PickHue { input: hop_repo::HueIn, output: f64 },
    #[serde(rename = "siblingLanes")]
    SiblingLanes {
        input: hop_repo::LanesIn,
        output: BTreeMap<String, hop_repo::RouteOut>,
    },
    #[serde(rename = "repoGroups")]
    RepoGroups {
        input: hop_repo::GroupsIn,
        output: hop_repo::GroupsOut,
    },
    #[serde(rename = "repoPlaces")]
    RepoPlaces {
        input: hop_repo::PlacesIn,
        output: hop_repo::PlacesOut,
    },
    #[serde(rename = "commitLanes")]
    CommitLanes {
        input: commit_lanes::LanesIn,
        output: commit_lanes::LanesOut,
    },
    #[serde(rename = "push")]
    Push {
        input: PushIn,
        output: BTreeMap<String, P>,
    },
    #[serde(rename = "unpile")]
    Unpile {
        input: UnpileIn,
        output: BTreeMap<String, P>,
    },
    #[serde(rename = "swap")]
    Swap { input: SwapIn, output: SwapOut },
    #[serde(rename = "centerSide")]
    CenterSide {
        input: Vec<CenterSideIn>,
        output: Vec<CS>,
    },
    #[serde(rename = "crossed")]
    Crossed {
        input: Vec<CrossedIn>,
        output: Vec<bool>,
    },
    #[serde(rename = "freeSpot")]
    FreeSpot {
        input: Vec<FreeSpotIn>,
        output: Vec<P>,
    },
    #[serde(rename = "samePoint")]
    SamePoint {
        input: Vec<SamePointIn>,
        output: Vec<bool>,
    },
    #[serde(rename = "stepAt")]
    StepAt {
        input: Vec<StepIn>,
        output: Vec<StepOut>,
    },
    #[serde(rename = "bendCenter")]
    BendCenter {
        input: Vec<BendIn>,
        output: Vec<BendOut>,
    },
    #[serde(rename = "edgeSides")]
    EdgeSides {
        input: Vec<EdgeIn>,
        output: Vec<EdgeOut>,
    },
    #[serde(rename = "labelHeading")]
    LabelHeading {
        input: Vec<HeadingIn>,
        output: Vec<HeadingOut>,
    },
    #[serde(rename = "followCard")]
    FollowCard {
        input: Vec<FollowIn>,
        output: Vec<Cam>,
    },
}

fn size(s: &S) -> Size {
    Size {
        width: s.width,
        height: s.height,
    }
}

fn direction_of(d: &Dir) -> Direction {
    match d {
        Dir::Down => Direction::Down,
        Dir::Right => Direction::Right,
        Dir::Up => Direction::Up,
        Dir::Left => Direction::Left,
    }
}

fn out_dir(d: Direction) -> Dir {
    match d {
        Direction::Down => Dir::Down,
        Direction::Right => Dir::Right,
        Direction::Up => Dir::Up,
        Direction::Left => Dir::Left,
    }
}

fn side(s: Sd) -> Side {
    match s {
        Sd::Top => Side::Top,
        Sd::Right => Side::Right,
        Sd::Bottom => Side::Bottom,
        Sd::Left => Side::Left,
    }
}

fn out_side(s: Side) -> Sd {
    match s {
        Side::Top => Sd::Top,
        Side::Right => Sd::Right,
        Side::Bottom => Sd::Bottom,
        Side::Left => Sd::Left,
    }
}

fn rect(r: &R) -> Rect {
    Rect {
        at: p(&r.at),
        size: size(&r.size),
    }
}

fn out_rect(r: Rect) -> R {
    R {
        at: out_p(r.at),
        size: S {
            width: r.size.width,
            height: r.size.height,
        },
    }
}

fn sibs(v: &[Sib]) -> Vec<Sibling> {
    v.iter()
        .map(|s| Sibling {
            key: s.key.clone(),
            rect: Rect {
                at: p(&s.at),
                size: size(&s.size),
            },
        })
        .collect()
}

fn points(m: BTreeMap<String, Point>) -> BTreeMap<String, P> {
    m.into_iter().map(|(k, q)| (k, out_p(q))).collect()
}

fn cs(c: &CS) -> CenterSide {
    CenterSide { x: c.x, y: c.y }
}

fn out_cs(c: CenterSide) -> CS {
    CS { x: c.x, y: c.y }
}

fn end(e: &EndIn) -> EdgeEnd {
    EdgeEnd {
        rect: rect(&e.rect),
        side: side(e.side),
    }
}

fn swapped(i: &SwapIn) -> SwapOut {
    let state = Swap {
        slot: rect(&i.state.slot),
        sides: i
            .state
            .sides
            .iter()
            .map(|(k, c)| (k.clone(), cs(c)))
            .collect(),
        at: i.state.at.iter().map(|(k, q)| (k.clone(), p(q))).collect(),
    };
    let out = layout::swap(&state, &sibs(&i.others), rect(&i.dragged));
    SwapOut {
        slot: out_rect(out.slot),
        sides: out.sides.into_iter().map(|(k, c)| (k, out_cs(c))).collect(),
        at: points(out.at),
    }
}

fn p(q: &P) -> Point {
    Point { x: q.x, y: q.y }
}

fn out_p(q: Point) -> P {
    P { x: q.x, y: q.y }
}

fn nested(i: &NestedIn) -> NestedOut {
    let groups: Vec<NestGroup> = i
        .groups
        .iter()
        .map(|g| NestGroup {
            parent: g.parent,
            card: g.card,
            members: g.members.clone(),
            empty: size(&g.empty),
        })
        .collect();
    let direction = direction_of(&i.direction);
    let out = layout::arrange_nested(
        &groups,
        &|id| i.parents.get(&id.to_string()).copied().flatten(),
        &|id| size(&i.sizes[&id.to_string()]),
        direction,
        Inset {
            top: i.inset.top,
            side: i.inset.side,
        },
        &|kid| {
            let key = match kid {
                Kid::Card(id) => format!("card:{id}"),
                Kid::Group(g) => format!("group:{g}"),
            };
            i.rank.get(&key).copied().unwrap_or(f64::INFINITY)
        },
    );
    NestedOut {
        cards: out
            .cards
            .into_iter()
            .map(|(id, q)| (id.to_string(), out_p(q)))
            .collect(),
        boxes: out
            .boxes
            .into_iter()
            .map(|Rect { at, size }| R {
                at: out_p(at),
                size: S {
                    width: size.width,
                    height: size.height,
                },
            })
            .collect(),
    }
}

fn settled(i: &SettleIn) -> BTreeMap<String, P> {
    let items: Vec<SettleItem> = i
        .items
        .iter()
        .map(|it| SettleItem {
            key: it.key.clone(),
            size: size(&it.size),
            lead: p(&it.lead),
            auto: p(&it.auto),
        })
        .collect();
    layout::settle(&items, &|key| i.placed.get(key).map(p), i.gap)
        .into_iter()
        .map(|(key, q)| (key, out_p(q)))
        .collect()
}

#[test]
fn layout_fixtures_match_the_web() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/layout");
    let mut names: Vec<_> = fs::read_dir(&dir)
        .expect("fixtures/layout")
        .map(|e| e.expect("entry").path())
        .filter(|f| f.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    for file in names {
        let case: Case =
            serde_json::from_str(&fs::read_to_string(&file).expect("read")).expect("parse");
        match case {
            Case::ArrangeNested { input, output } => {
                assert_eq!(nested(&input), output, "{}", file.display())
            }
            Case::Settle { input, output } => {
                assert_eq!(settled(&input), output, "{}", file.display())
            }
            Case::HopParts { input, output } => {
                assert_eq!(hop_repo::parts(&input), output, "{}", file.display())
            }
            Case::PickHue { input, output } => {
                assert_eq!(hop_repo::hue(&input), output, "{}", file.display())
            }
            Case::SiblingLanes { input, output } => {
                assert_eq!(hop_repo::lanes(&input), output, "{}", file.display())
            }
            Case::RepoGroups { input, output } => {
                assert_eq!(hop_repo::groups(&input), output, "{}", file.display())
            }
            Case::RepoPlaces { input, output } => {
                assert_eq!(hop_repo::places(&input), output, "{}", file.display())
            }
            Case::CommitLanes { input, output } => {
                assert_eq!(
                    commit_lanes::commit_lanes(&input),
                    output,
                    "{}",
                    file.display()
                )
            }
            Case::Push { input, output } => {
                let got = layout::push(
                    &sibs(&input.siblings),
                    rect(&input.mover),
                    &input.targets,
                    input.gap,
                );
                assert_eq!(points(got), output, "{}", file.display())
            }
            Case::Unpile { input, output } => {
                let got = layout::unpile(&sibs(&input.siblings), input.gap);
                assert_eq!(points(got), output, "{}", file.display())
            }
            Case::Swap { input, output } => {
                assert_eq!(swapped(&input), output, "{}", file.display())
            }
            Case::CenterSide { input, output } => {
                let got: Vec<CS> = input
                    .iter()
                    .map(|c| out_cs(layout::center_side(rect(&c.dragged), rect(&c.rect))))
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::Crossed { input, output } => {
                let got: Vec<bool> = input
                    .iter()
                    .map(|c| layout::crossed(cs(&c.before), rect(&c.dragged), rect(&c.rect)))
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::FreeSpot { input, output } => {
                let got: Vec<P> = input
                    .iter()
                    .map(|c| {
                        let others: Vec<Rect> = c.others.iter().map(rect).collect();
                        out_p(layout::free_spot(rect(&c.rect), &others, c.gap))
                    })
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::SamePoint { input, output } => {
                let got: Vec<bool> = input
                    .iter()
                    .map(|c| layout::same_point(p(&c.a), p(&c.b)))
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::StepAt { input, output } => {
                let got: Vec<StepOut> = input
                    .iter()
                    .map(|c| StepOut {
                        step: layout::step_at(
                            rect(&c.parent),
                            rect(&c.child),
                            direction_of(&c.direction),
                            c.bend,
                        ),
                    })
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::BendCenter { input, output } => {
                let got: Vec<BendOut> = input
                    .iter()
                    .map(|c| {
                        let b = layout::bend_center(
                            rect(&c.parent),
                            c.bend,
                            direction_of(&c.direction),
                        );
                        BendOut { x: b.x, y: b.y }
                    })
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::EdgeSides { input, output } => {
                let got: Vec<EdgeOut> = input
                    .iter()
                    .map(|c| {
                        let t = layout::edge_sides(
                            rect(&c.parent),
                            rect(&c.child),
                            direction_of(&c.direction),
                        );
                        EdgeOut {
                            parent: out_side(t.parent),
                            child: out_side(t.child),
                        }
                    })
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
            Case::LabelHeading { input, output } => {
                let got: Vec<Dir> = input
                    .iter()
                    .map(|c| out_dir(layout::label_heading(end(&c.from), end(&c.to), c.step)))
                    .collect();
                let want: Vec<Dir> = output.into_iter().map(|h| h.heading).collect();
                assert_eq!(got, want, "{}", file.display())
            }
            Case::FollowCard { input, output } => {
                let got: Vec<Cam> = input
                    .iter()
                    .map(|c| {
                        let k = layout::follow_card(
                            p(&c.old),
                            p(&c.now),
                            Camera {
                                x: c.camera.x,
                                y: c.camera.y,
                                zoom: c.camera.zoom,
                            },
                        );
                        Cam {
                            x: k.x,
                            y: k.y,
                            zoom: k.zoom,
                        }
                    })
                    .collect();
                assert_eq!(got, output, "{}", file.display())
            }
        }
    }
}
