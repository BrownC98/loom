use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui::{Context, Entity};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use api::repos::{FolderList, Locations};
use api::settings::places::{DistroView, ReachView};

use crate::link;
use crate::problem::Failure;
use crate::store::Store;
use crate::views::cardlink;

const CLONE_LIMIT: Duration = Duration::from_secs(3600);
const SSH_SETUP_LIMIT: Duration = Duration::from_secs(600);

#[derive(Clone, Default)]
pub struct Inflight(Arc<AtomicUsize>);

pub struct Held(Arc<AtomicUsize>);

impl Inflight {
    pub fn hold(&self) -> Held {
        self.0.fetch_add(1, Ordering::SeqCst);
        Held(self.0.clone())
    }

    pub fn idle(&self) -> bool {
        self.0.load(Ordering::SeqCst) == 0
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn run<V: 'static, T: Send + 'static>(
    store: &Entity<Store>,
    cx: &mut Context<V>,
    work: impl FnOnce(&str) -> Result<T, Failure> + Send + 'static,
    done: impl FnOnce(&mut V, Result<T, String>, &mut Context<V>) + 'static,
) {
    cardlink::run(store, cx, work, done).detach();
}

fn body(value: Value) -> Option<String> {
    serde_json::to_string(&value).ok()
}

fn sent<T: DeserializeOwned>(base: &str, path: &str, value: Value) -> Result<T, Failure> {
    link::post(base, path, body(value))
}

pub fn remove_repo(base: &str, repo: u32) -> Result<(), Failure> {
    link::delete(base, &format!("/api/repos/{repo}"))
}

pub fn rename_worktree(base: &str, repo: u32, path: &str, name: &str) -> Result<(), Failure> {
    link::put(
        base,
        &format!("/api/repos/{repo}/worktrees/name"),
        &json!({ "path": path, "name": name }),
    )
}

pub fn add_repo(base: &str, path: &str) -> Result<u32, Failure> {
    added(sent(base, "/api/repos", json!({ "path": path }))?)
}

pub fn clone_repo(base: &str, url: &str, parent: &str) -> Result<u32, Failure> {
    added(link::post_within(
        base,
        "/api/repos/clone",
        body(json!({ "url": url, "parent": parent })),
        CLONE_LIMIT,
    )?)
}

pub fn create_repo(base: &str, parent: &str, name: &str) -> Result<u32, Failure> {
    added(sent(
        base,
        "/api/repos/create",
        json!({ "parent": parent, "name": name }),
    )?)
}

fn added(repo: Value) -> Result<u32, Failure> {
    repo.get("id")
        .and_then(Value::as_u64)
        .map(|id| id as u32)
        .ok_or_else(|| Failure::Other("repo: no id".to_owned()))
}

pub fn folders(base: &str, path: Option<&str>) -> Result<FolderList, Failure> {
    link::read(
        base,
        &format!("/api/folders{}", link::query(&[("path", path)])),
    )
}

pub fn locations(base: &str) -> Result<Locations, Failure> {
    link::read(base, "/api/locations")
}

pub fn distro_home(base: &str, name: &str) -> Result<String, Failure> {
    link::distro(base, name).map(|DistroView { home, .. }| home)
}

pub fn connect_ssh(base: &str, host: &str) -> Result<ReachView, Failure> {
    link::post_within(
        base,
        &format!("/api/ssh/{}", link::escape(host)),
        None,
        SSH_SETUP_LIMIT,
    )
}

pub fn release_ssh(base: &str, host: &str) -> Result<(), Failure> {
    link::post::<Value>(
        base,
        &format!("/api/ssh/{}/release", link::escape(host)),
        None,
    )
    .map(|_| ())
}
