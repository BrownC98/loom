#[path = "../src/canvas/hops.rs"]
mod hops;
#[path = "../src/canvas/layout.rs"]
mod layout;
#[path = "../src/canvas/repos.rs"]
mod repos;

#[path = "layout/hop_repo.rs"]
mod hop_repo;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use layout::{Direction, Inset, Kid, NestGroup, Point, Rect, SettleItem, Size};
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
#[serde(rename_all = "camelCase")]
enum Dir {
    Down,
    Right,
    Up,
    Left,
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
}

fn size(s: &S) -> Size {
    Size {
        width: s.width,
        height: s.height,
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
    let direction = match i.direction {
        Dir::Down => Direction::Down,
        Dir::Right => Direction::Right,
        Dir::Up => Direction::Up,
        Dir::Left => Direction::Left,
    };
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
        }
    }
}
