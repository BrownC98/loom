use super::tree::{Body, Row};
use crate::strings::{Lang, Str};
use crate::views::icons::Lucide;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Bookmark(u32, bool),
    Rename,
    RemoveNode(u32),
    NewNode,
    NewWorktree,
    RemoveRepo,
    DeleteWorktree,
}

pub struct Button {
    pub act: Act,
    pub glyph: Lucide,
    pub label: String,
    pub disabled: bool,
    pub pressed: bool,
}

impl Button {
    pub fn danger(&self) -> bool {
        matches!(
            self.act,
            Act::RemoveNode(_) | Act::RemoveRepo | Act::DeleteWorktree
        )
    }
}

fn plain(act: Act, glyph: Lucide, label: &str) -> Button {
    Button {
        act,
        glyph,
        label: label.to_owned(),
        disabled: false,
        pressed: false,
    }
}

fn bookmark(id: u32, marked: bool, lang: Lang) -> Button {
    Button {
        act: Act::Bookmark(id, marked),
        glyph: Lucide::Bookmark,
        label: if marked {
            Str::BookmarkRemove
        } else {
            Str::BookmarkAdd
        }
        .text(lang)
        .to_owned(),
        disabled: false,
        pressed: marked,
    }
}

pub fn can_rename(row: &Row) -> bool {
    match &row.body {
        Body::Node { .. } => true,
        Body::Group { branch, .. } => branch.is_some(),
        _ => false,
    }
}

pub fn remove_of(row: &Row) -> Option<Act> {
    match &row.body {
        Body::Node { id, .. } => Some(Act::RemoveNode(*id)),
        Body::Group { linked: true, .. } => Some(Act::DeleteWorktree),
        Body::Repo { .. } => Some(Act::RemoveRepo),
        _ => None,
    }
}

pub fn buttons(row: &Row, lang: Lang) -> Vec<Button> {
    let mut out = Vec::new();
    match &row.body {
        Body::Node { id, marked, .. } => out.push(bookmark(*id, *marked, lang)),
        Body::Mark { id, .. } => out.push(bookmark(*id, true, lang)),
        Body::Group { .. } => out.push(plain(
            Act::NewNode,
            Lucide::Plus,
            Str::GraphNewNode.text(lang),
        )),
        Body::Repo { no_worktree, .. } => {
            let label = Str::RepoNewWorktree.text(lang);
            out.push(Button {
                act: Act::NewWorktree,
                glyph: Lucide::GitBranchPlus,
                label: match no_worktree {
                    None => label.to_owned(),
                    Some(why) => format!("{label}: {}", why.text(lang)),
                },
                disabled: no_worktree.is_some(),
                pressed: false,
            });
        }
    }
    if matches!(row.key, super::tree::Key::Mark(_)) {
        return out;
    }
    if can_rename(row) {
        out.push(plain(
            Act::Rename,
            Lucide::Pencil,
            Str::RenameAction.text(lang),
        ));
    }
    match remove_of(row) {
        Some(act @ Act::RemoveNode(_)) => {
            out.push(plain(act, Lucide::Trash2, Str::NodeRemove.text(lang)))
        }
        Some(act @ Act::DeleteWorktree) => out.push(plain(
            act,
            Lucide::Trash2,
            Str::DeleteWorktreeAction.text(lang),
        )),
        Some(act) => out.push(plain(act, Lucide::ListX, Str::TreeRemoveRepo.text(lang))),
        None => {}
    }
    out
}
