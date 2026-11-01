use api::graph::CardView;
use api::repos::RepoView;
use gpui::{App, AppContext, Context, Entity, Task};
use serde::Serialize;
use serde_json::Value;

use crate::link;
use crate::problem::Failure;
use crate::store::Store;
use crate::strings::Lang;

#[derive(Serialize)]
struct Rename<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct NewWorktree<'a> {
    branch: &'a str,
    base: &'a str,
}

pub fn steer_queued(base: &str, node: u32) -> Result<(), Failure> {
    link::post::<Value>(base, &format!("/api/nodes/{node}/steer-queued"), None).map(|_| ())
}

pub fn set_card_view(base: &str, node: u32, view: CardView) -> Result<(), Failure> {
    link::put(base, &format!("/api/nodes/{node}/card-view"), &view)
}

pub fn clear_card_view(base: &str, node: u32) -> Result<(), Failure> {
    link::delete(base, &format!("/api/nodes/{node}/card-view"))
}

pub fn relaunch(base: &str, node: u32) -> Result<(), Failure> {
    link::post::<Value>(base, &format!("/api/nodes/{node}/relaunch"), None).map(|_| ())
}

pub fn remove_node(base: &str, node: u32) -> Result<(), Failure> {
    link::delete(base, &format!("/api/nodes/{node}"))
}

pub fn rename(base: &str, node: u32, name: &str) -> Result<(), Failure> {
    link::put(base, &format!("/api/nodes/{node}/name"), &Rename { name })
}

pub fn create_worktree(
    base: &str,
    repo: u32,
    branch: &str,
    from: &str,
) -> Result<RepoView, Failure> {
    let body = serde_json::to_string(&NewWorktree { branch, base: from }).ok();
    link::post(base, &format!("/api/repos/{repo}/worktrees"), body)
}

pub fn worktree_changes(base: &str, repo: u32, path: &str) -> Result<Vec<String>, Failure> {
    link::read(
        base,
        &format!(
            "/api/repos/{repo}/worktrees/changes{}",
            link::query(&[("path", Some(path))])
        ),
    )
}

pub fn delete_worktree(
    base: &str,
    repo: u32,
    path: &str,
    delete_branch: bool,
) -> Result<(), Failure> {
    let flag = delete_branch.to_string();
    link::delete(
        base,
        &format!(
            "/api/repos/{repo}/worktrees{}",
            link::query(&[("path", Some(path)), ("deleteBranch", Some(&flag))])
        ),
    )
}

pub fn run<T, V>(
    store: &Entity<Store>,
    cx: &mut Context<V>,
    work: impl FnOnce(&str) -> Result<T, Failure> + Send + 'static,
    done: impl FnOnce(&mut V, Result<T, String>, &mut Context<V>) + 'static,
) -> Task<()>
where
    T: Send + 'static,
    V: 'static,
{
    let base = store.read(cx).base().map(str::to_owned);
    let status = store.read(cx).status().to_owned();
    cx.spawn(async move |view, cx| {
        let result = match base {
            Some(base) => {
                let lang = cx.update(|app: &mut App| *app.global::<Lang>());
                cx.background_spawn(async move { work(&base) })
                    .await
                    .map_err(|failure| failure.text(lang))
            }
            None => Err(status),
        };
        let _ = view.update(cx, |view, cx| done(view, result, cx));
    })
}
