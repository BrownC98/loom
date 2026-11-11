use std::collections::HashSet;

use api::graph::waits::WaitTone;
use api::graph::{NodeState, NodeView, ProcessState};
use api::profile::AgentView;
use api::repos::PlaceView;

use crate::canvas::repos::{
    Group, GroupKind, bookmarked, group_parents, groups_of, in_group, place_name,
};
use crate::link::Snapshot;
use crate::strings::{Lang, Str};
use crate::views::dock::questions::kind_label;
use crate::views::status::{self, Tone};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Repo(u32),
    Group(u32, String),
    Node(u32),
    Mark(u32),
}

impl Key {
    pub fn node(&self) -> Option<u32> {
        match self {
            Key::Node(id) | Key::Mark(id) => Some(*id),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Waiting,
    Working,
    Idle,
    Done,
    Failed,
}

pub struct Tag {
    pub text: String,
    pub tone: Tone,
    pub tip: Option<String>,
}

pub enum Body {
    Repo {
        name: String,
        path: String,
        full: String,
        chip: Option<Chip>,
        no_worktree: Option<Str>,
    },
    Group {
        label: String,
        tag: Option<&'static str>,
        branch: Option<String>,
        linked: bool,
    },
    Node {
        id: u32,
        name: String,
        line: String,
        agent: Option<String>,
        agent_label: String,
        dot: Dot,
        tags: Vec<Tag>,
        unread: u32,
        from: Option<String>,
        marked: bool,
    },
    Mark {
        id: u32,
        name: String,
        place: String,
        dot: Dot,
    },
}

pub struct Chip {
    pub text: String,
    pub title: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Context,
    Hit,
    Current,
}

pub struct Row {
    pub key: Key,
    pub depth: usize,
    pub parent: Option<Key>,
    pub branch: bool,
    pub open: bool,
    pub found: Option<Found>,
    pub span: Option<(usize, usize)>,
    pub body: Body,
}

pub struct Built {
    pub marks: Vec<Row>,
    pub tree: Vec<Row>,
    pub hits: Vec<Key>,
}

impl Built {
    pub fn rows(&self, marks_open: bool) -> impl Iterator<Item = &Row> {
        self.marks
            .iter()
            .filter(move |_| marks_open)
            .chain(self.tree.iter())
    }
}

struct Item {
    key: Key,
    name: Option<String>,
    body: Body,
    children: Vec<Item>,
}

fn dot_of(view: &NodeView) -> Dot {
    if view.waits.iter().any(status::asking) {
        return Dot::Waiting;
    }
    match view.state {
        NodeState::Running => process_dot(&view.process),
        NodeState::Done => Dot::Done,
        NodeState::SetupFailed { .. } => Dot::Failed,
    }
}

fn process_dot(process: &ProcessState) -> Dot {
    match process {
        ProcessState::Working => Dot::Working,
        ProcessState::Idle | ProcessState::Asleep => Dot::Idle,
        ProcessState::Blocked { before } => process_dot(before),
        ProcessState::Stopped { .. } => Dot::Failed,
    }
}

fn tags(view: &NodeView, lang: Lang) -> Vec<Tag> {
    let asks: Vec<_> = view.waits.iter().filter(|w| status::asking(w)).collect();
    let dones = view
        .waits
        .iter()
        .filter(|w| matches!(w.tone, WaitTone::Done))
        .count();
    let mut out = Vec::new();
    if let Some(oldest) = asks.iter().min_by_key(|w| w.wait.since) {
        out.push(Tag {
            text: format!("{} {}", kind_label(oldest.wait.kind).text(lang), asks.len()),
            tone: Tone::Ask,
            tip: None,
        });
    }
    if dones > 0 {
        out.push(Tag {
            text: Str::TreeDone.with(lang, &[("n", &dones.to_string())]),
            tone: Tone::Done,
            tip: None,
        });
    }
    let label = Str::NodeStatusPopLabel.with(lang, &[("name", &view.name)]);
    let state = status::chip(&view.status, lang).map(|c| Tag {
        text: c.text,
        tone: c.tone,
        tip: Some(label),
    });
    out.extend(state);
    out
}

fn agent_label(agents: &[AgentView], view: &NodeView, lang: Lang) -> String {
    match &view.agent {
        Some(id) => agents
            .iter()
            .find(|a| a.id == *id)
            .map_or_else(|| id.0.clone(), |a| a.label.clone()),
        None => Str::NodeTerminal.text(lang).to_owned(),
    }
}

fn model_line(view: &NodeView, label: &str) -> String {
    let model = [view.model.as_deref(), view.effort.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    if model.is_empty() {
        label.to_owned()
    } else {
        model
    }
}

struct Context<'a> {
    views: Vec<&'a NodeView>,
    agents: &'a [AgentView],
    lang: Lang,
}

type Note<'a> = &'a dyn Fn(&NodeView) -> Option<String>;

fn node_items(cx: &Context, members: &[&NodeView], parent: Option<u32>, note: Note) -> Vec<Item> {
    let mut picked: Vec<&NodeView> = members
        .iter()
        .copied()
        .filter(|v| match parent {
            None => !members.iter().any(|p| Some(p.id) == v.parent),
            Some(id) => v.parent.map(|p| p.0) == Some(id),
        })
        .collect();
    picked.sort_by_key(|v| v.id.0);
    picked
        .into_iter()
        .map(|v| {
            let label = agent_label(cx.agents, v, cx.lang);
            Item {
                key: Key::Node(v.id.0),
                name: Some(v.name.clone()),
                body: Body::Node {
                    id: v.id.0,
                    name: v.name.clone(),
                    line: model_line(v, &label),
                    agent: v.agent.as_ref().map(|a| a.0.clone()),
                    agent_label: label,
                    dot: dot_of(v),
                    tags: tags(v, cx.lang),
                    unread: v.unread,
                    from: note(v),
                    marked: v.bookmark.is_some(),
                },
                children: node_items(cx, members, Some(v.id.0), note),
            }
        })
        .collect()
}

fn group_items(
    cx: &Context,
    snapshot: &Snapshot,
    repo: &api::repos::RepoView,
    groups: &[Group],
    parents: &[Option<usize>],
    at: Option<usize>,
) -> Vec<Item> {
    groups
        .iter()
        .enumerate()
        .filter(|(i, _)| parents[*i] == at)
        .map(|(i, group)| {
            let note = |view: &NodeView| {
                let parent = snapshot.nodes.iter().find(|p| Some(p.id) == view.parent)?;
                let from = groups
                    .iter()
                    .position(|g| g.path == parent.place.worktree)?;
                (from != i && Some(from) != parents[i]).then(|| {
                    Str::TreeParent.with(
                        cx.lang,
                        &[("node", &parent.name), ("group", &groups[from].label)],
                    )
                })
            };
            let mut children = node_items(cx, &in_group(&cx.views, group), None, &note);
            children.extend(group_items(cx, snapshot, repo, groups, parents, Some(i)));
            let linked = group.kind == GroupKind::Linked;
            Item {
                key: Key::Group(repo.id.0, group.path.clone()),
                name: Some(group.label.clone()),
                body: Body::Group {
                    label: group.label.clone(),
                    branch: repo
                        .worktrees
                        .iter()
                        .find(|w| w.path == group.path)
                        .and_then(|w| w.branch.clone())
                        .filter(|_| linked),
                    linked,
                    tag: match group.kind {
                        GroupKind::Linked => None,
                        GroupKind::Original => Some(Str::WorktreeOriginal.text(cx.lang)),
                        GroupKind::Unborn => Some(Str::WorktreeUnborn.text(cx.lang)),
                        GroupKind::Plain => Some(Str::WorktreePlain.text(cx.lang)),
                    },
                },
                children,
            }
        })
        .collect()
}

fn chip(place: &PlaceView, lang: Lang) -> Option<Chip> {
    match place {
        PlaceView::Pc => None,
        PlaceView::Wsl { distro } => Some(Chip {
            text: Str::PlaceWsl.text(lang).to_owned(),
            title: Str::PlaceWslDistro.with(lang, &[("distro", distro)]),
        }),
        PlaceView::Ssh { host } => {
            let title = Str::PlaceSshHost.with(lang, &[("host", host)]);
            Some(Chip {
                text: title.clone(),
                title,
            })
        }
    }
}

fn hit(name: &Option<String>, needle: &str) -> bool {
    name.as_ref()
        .is_some_and(|n| n.to_lowercase().contains(needle))
}

fn keep(items: Vec<Item>, needle: &str) -> Vec<Item> {
    items
        .into_iter()
        .filter_map(|mut item| {
            item.children = keep(std::mem::take(&mut item.children), needle);
            (hit(&item.name, needle) || !item.children.is_empty()).then_some(item)
        })
        .collect()
}

fn collect(items: &[Item], needle: &str, hits: &mut Vec<Key>) {
    for item in items {
        if hit(&item.name, needle) {
            hits.push(item.key.clone());
        }
        collect(&item.children, needle, hits);
    }
}

fn span(name: &Option<String>, needle: &str) -> Option<(usize, usize)> {
    let name = name.as_ref()?;
    let lower = name.to_lowercase();
    if lower.len() != name.len() {
        return None;
    }
    lower.find(needle).map(|from| (from, from + needle.len()))
}

struct Walk<'a> {
    closed: &'a HashSet<Key>,
    needle: &'a str,
    hits: &'a [Key],
    at: usize,
}

fn flatten(items: Vec<Item>, depth: usize, parent: Option<&Key>, walk: &Walk, out: &mut Vec<Row>) {
    let searching = !walk.needle.is_empty();
    for item in items {
        let open = searching || !walk.closed.contains(&item.key);
        let found = searching.then(|| match walk.hits.iter().position(|k| *k == item.key) {
            None => Found::Context,
            Some(i) if i == walk.at => Found::Current,
            Some(_) => Found::Hit,
        });
        let shown = match found {
            Some(Found::Hit | Found::Current) => span(&item.name, walk.needle),
            _ => None,
        };
        out.push(Row {
            key: item.key.clone(),
            depth,
            parent: parent.cloned(),
            branch: !item.children.is_empty(),
            open,
            found,
            span: shown,
            body: item.body,
        });
        if open {
            flatten(item.children, depth + 1, Some(&item.key), walk, out);
        }
    }
}

pub fn build(
    snapshot: &Snapshot,
    agents: &[AgentView],
    lang: Lang,
    closed: &HashSet<Key>,
    query: &str,
    at: usize,
) -> Built {
    let cx = Context {
        views: snapshot.nodes.iter().collect(),
        agents,
        lang,
    };
    let mut items: Vec<Item> = snapshot
        .repos
        .iter()
        .map(|repo| {
            let groups = groups_of(repo, &cx.views);
            let parents = group_parents(&groups, &cx.views);
            Item {
                key: Key::Repo(repo.id.0),
                name: None,
                body: Body::Repo {
                    name: repo.name.clone(),
                    path: repo.shown_path.clone(),
                    full: repo.path.clone(),
                    chip: chip(&repo.place, lang),
                    no_worktree: crate::views::card::worktree_block(repo),
                },
                children: group_items(&cx, snapshot, repo, &groups, &parents, None),
            }
        })
        .collect();
    let needle = query.trim().to_lowercase();
    let searching = !needle.is_empty();
    let mut hits = Vec::new();
    if searching {
        items = keep(items, &needle);
        collect(&items, &needle, &mut hits);
    }
    let mut tree = Vec::new();
    let walk = Walk {
        closed,
        needle: &needle,
        hits: &hits,
        at,
    };
    flatten(items, 0, None, &walk, &mut tree);
    let marks = bookmarked(&cx.views)
        .into_iter()
        .map(|v| Row {
            key: Key::Mark(v.id.0),
            depth: 0,
            parent: None,
            branch: false,
            open: false,
            found: None,
            span: None,
            body: Body::Mark {
                id: v.id.0,
                name: v.name.clone(),
                place: place_name(&snapshot.repos, &cx.views, v),
                dot: dot_of(v),
            },
        })
        .collect();
    Built { marks, tree, hits }
}
