use std::collections::HashSet;

use api::graph::waits::{WaitKind, WaitTone};
use api::graph::{NodeState, NodeView};

use crate::canvas::repos::{
    Group, GroupKind, bookmarked, group_parents, groups_of, in_group, place_name,
};
use crate::link::Snapshot;
use crate::strings::{Lang, Str};

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
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ask,
    Done,
    Failed,
}

pub struct Tag {
    pub text: String,
    pub tone: Tone,
}

pub enum Body {
    Repo {
        name: String,
        path: String,
    },
    Group {
        label: String,
        tag: Option<&'static str>,
    },
    Node {
        id: u32,
        name: String,
        line: String,
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
    if view.waits.iter().any(|w| matches!(w.tone, WaitTone::Ask)) {
        return Dot::Waiting;
    }
    match view.state {
        NodeState::Running => Dot::Running,
        NodeState::Done => Dot::Done,
        NodeState::Exited { .. } | NodeState::SetupFailed { .. } => Dot::Failed,
    }
}

fn kind_text(kind: &WaitKind) -> Str {
    match kind {
        WaitKind::Question => Str::WaitQuestion,
        WaitKind::Approval => Str::WaitApproval,
        WaitKind::Trust => Str::WaitTrust,
        WaitKind::Auth => Str::WaitAuth,
        WaitKind::Input => Str::WaitInput,
        WaitKind::Done => Str::WaitDone,
    }
}

pub fn state_text(state: &NodeState, lang: Lang) -> String {
    match state {
        NodeState::Running => Str::NodeStateRunning.text(lang).to_owned(),
        NodeState::Done => Str::NodeStateDone.text(lang).to_owned(),
        NodeState::Exited { code } => Str::NodeStateExited.with(
            lang,
            &[(
                "code",
                &code.map_or_else(|| "?".to_owned(), |c| c.to_string()),
            )],
        ),
        NodeState::SetupFailed { .. } => Str::NodeStateSetupFailed.text(lang).to_owned(),
    }
}

fn tags(view: &NodeView, lang: Lang) -> Vec<Tag> {
    let asks: Vec<_> = view
        .waits
        .iter()
        .filter(|w| matches!(w.tone, WaitTone::Ask))
        .collect();
    let dones = view.waits.len() - asks.len();
    if view.waits.is_empty() {
        return match view.state {
            NodeState::Exited { .. } | NodeState::SetupFailed { .. } => vec![Tag {
                text: state_text(&view.state, lang),
                tone: Tone::Failed,
            }],
            _ => Vec::new(),
        };
    }
    let mut out = Vec::new();
    if let Some(oldest) = asks.iter().min_by_key(|w| w.wait.since) {
        out.push(Tag {
            text: format!("{} {}", kind_text(&oldest.wait.kind).text(lang), asks.len()),
            tone: Tone::Ask,
        });
    }
    if dones > 0 {
        out.push(Tag {
            text: Str::TreeDone.with(lang, &[("n", &dones.to_string())]),
            tone: Tone::Done,
        });
    }
    out
}

fn model_line(view: &NodeView, lang: Lang) -> String {
    let model = [view.model.as_deref(), view.effort.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    if !model.is_empty() {
        return model;
    }
    match &view.agent {
        Some(agent) => agent.0.clone(),
        None => Str::NodeTerminal.text(lang).to_owned(),
    }
}

struct Context<'a> {
    views: Vec<&'a NodeView>,
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
        .map(|v| Item {
            key: Key::Node(v.id.0),
            name: Some(v.name.clone()),
            body: Body::Node {
                id: v.id.0,
                name: v.name.clone(),
                line: model_line(v, cx.lang),
                dot: dot_of(v),
                tags: tags(v, cx.lang),
                unread: v.unread,
                from: note(v),
                marked: v.bookmark.is_some(),
            },
            children: node_items(cx, members, Some(v.id.0), note),
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
            Item {
                key: Key::Group(repo.id.0, group.path.clone()),
                name: Some(group.label.clone()),
                body: Body::Group {
                    label: group.label.clone(),
                    tag: match group.kind {
                        GroupKind::Linked => None,
                        GroupKind::Original => Some(Str::WorktreeOriginal.text(cx.lang)),
                        GroupKind::Unborn => Some(Str::WorktreeUnborn.text(cx.lang)),
                    },
                },
                children,
            }
        })
        .collect()
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

#[allow(clippy::too_many_arguments)]
fn flatten(
    items: Vec<Item>,
    depth: usize,
    parent: Option<&Key>,
    closed: &HashSet<Key>,
    searching: bool,
    hits: &[Key],
    at: usize,
    out: &mut Vec<Row>,
) {
    for item in items {
        let open = searching || !closed.contains(&item.key);
        let found = searching.then(|| match hits.iter().position(|k| *k == item.key) {
            None => Found::Context,
            Some(i) if i == at => Found::Current,
            Some(_) => Found::Hit,
        });
        out.push(Row {
            key: item.key.clone(),
            depth,
            parent: parent.cloned(),
            branch: !item.children.is_empty(),
            open,
            found,
            body: item.body,
        });
        if open {
            flatten(
                item.children,
                depth + 1,
                Some(&item.key),
                closed,
                searching,
                hits,
                at,
                out,
            );
        }
    }
}

pub fn build(
    snapshot: &Snapshot,
    lang: Lang,
    closed: &HashSet<Key>,
    query: &str,
    at: usize,
) -> Built {
    let cx = Context {
        views: snapshot.nodes.iter().collect(),
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
    flatten(items, 0, None, closed, searching, &hits, at, &mut tree);
    let marks = bookmarked(&cx.views)
        .into_iter()
        .map(|v| Row {
            key: Key::Mark(v.id.0),
            depth: 0,
            parent: None,
            branch: false,
            open: false,
            found: None,
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
