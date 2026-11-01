#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use api::graph::NodeView;
use api::ids::RepoId;
use api::repos::RepoView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Original,
    Unborn,
    Linked,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub path: String,
    pub label: String,
    pub kind: GroupKind,
    pub branch: Option<String>,
}

pub fn folder_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

pub fn groups_of(repo: &RepoView, views: &[&NodeView]) -> Vec<Group> {
    let mut groups: Vec<Group> = repo
        .worktrees
        .iter()
        .map(|w| Group {
            path: w.path.clone(),
            label: w.branch.clone().unwrap_or_else(|| folder_name(&w.path)),
            kind: if !w.original {
                GroupKind::Linked
            } else if repo.has_commits {
                GroupKind::Original
            } else {
                GroupKind::Unborn
            },
            branch: w.branch.clone(),
        })
        .collect();
    for v in views {
        if v.place.repo == repo.id && !groups.iter().any(|g| g.path == v.place.worktree) {
            groups.push(Group {
                path: v.place.worktree.clone(),
                label: folder_name(&v.place.worktree),
                kind: GroupKind::Linked,
                branch: None,
            });
        }
    }
    groups
}

pub fn place_name(repos: &[RepoView], views: &[&NodeView], view: &NodeView) -> String {
    let Some(repo) = repos.iter().find(|r| r.id == view.place.repo) else {
        return String::new();
    };
    match groups_of(repo, views)
        .iter()
        .find(|g| g.path == view.place.worktree)
    {
        Some(group) => format!("{} · {}", repo.name, group.label),
        None => repo.name.clone(),
    }
}

pub fn bookmarked<'a>(views: &[&'a NodeView]) -> Vec<&'a NodeView> {
    let mut marked: Vec<&NodeView> = views
        .iter()
        .copied()
        .filter(|v| v.bookmark.is_some())
        .collect();
    marked.sort_by_key(|v| v.bookmark);
    marked
}

pub fn split_chips(groups: Vec<Group>, views: &[&NodeView]) -> (Vec<Group>, Vec<Group>) {
    if groups.iter().all(|g| g.kind == GroupKind::Linked) {
        return (groups, Vec::new());
    }
    groups.into_iter().partition(|g| {
        !(g.kind == GroupKind::Linked && !views.iter().any(|v| v.place.worktree == g.path))
    })
}

pub fn in_group<'a>(views: &[&'a NodeView], group: &Group) -> Vec<&'a NodeView> {
    views
        .iter()
        .copied()
        .filter(|v| v.place.worktree == group.path)
        .collect()
}

pub fn group_parents(groups: &[Group], views: &[&NodeView]) -> Vec<Option<usize>> {
    let group_of: HashMap<u32, usize> = views
        .iter()
        .filter_map(|v| {
            Some((
                v.id.0,
                groups.iter().position(|g| g.path == v.place.worktree)?,
            ))
        })
        .collect();
    let parents: Vec<Option<usize>> = groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            views
                .iter()
                .filter(|v| v.place.worktree == group.path)
                .filter_map(|v| group_of.get(&v.parent?.0).copied())
                .filter(|&p| p != i)
                .min()
        })
        .collect();
    break_cycles(&parents)
}

pub fn attach_cards(
    groups: &[Group],
    views: &[&NodeView],
    parents: &[Option<usize>],
) -> Vec<Option<u32>> {
    let by_id: HashMap<u32, &NodeView> = views.iter().map(|v| (v.id.0, *v)).collect();
    groups
        .iter()
        .zip(parents)
        .map(|(group, parent)| {
            let parent = (*parent)?;
            views
                .iter()
                .filter(|v| v.place.worktree == group.path)
                .find_map(|v| {
                    let up = v.parent?.0;
                    (by_id.get(&up)?.place.worktree == groups[parent].path).then_some(up)
                })
        })
        .collect()
}

fn break_cycles(parents: &[Option<usize>]) -> Vec<Option<usize>> {
    let mut fixed = parents.to_vec();
    for i in 0..fixed.len() {
        let mut seen = HashSet::from([i]);
        let mut at = fixed[i];
        while let Some(a) = at {
            if !seen.insert(a) {
                let mut ring = vec![a];
                let mut k = fixed[a];
                while let Some(next) = k.filter(|&next| next != a) {
                    ring.push(next);
                    k = fixed[next];
                }
                if let Some(&last) = ring.iter().max() {
                    fixed[last] = None;
                }
                break;
            }
            at = fixed[a];
        }
    }
    fixed
}

pub fn chosen_repo(repos: &[RepoView], saved: Option<RepoId>) -> Option<&RepoView> {
    repos
        .iter()
        .find(|r| Some(r.id) == saved)
        .or_else(|| repos.first())
}
