use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::App;
use crate::graph::TaskPins;
use crate::task;
use crate::worktree::merge::{TIDY_LOGS, script_within};
use api::graph::events::Entry;
use api::ids::RepoId;

const KEEP_FOR: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const TIDY_LIMIT: Duration = Duration::from_secs(10 * 60);
const FOLDERS_PER_RUN: usize = 100;

#[derive(Default)]
struct Tidied {
    folders: u32,
    logs: u32,
    kib: u64,
}

pub(super) async fn run(app: Arc<App>) {
    let Ok(_one) = app.tidying.try_lock() else {
        return;
    };
    let mut tidied = Tidied::default();
    if tokio::time::timeout(TIDY_LIMIT, tidy(&app, &mut tidied))
        .await
        .is_err()
    {
        crate::log::logln!("loom: tidy: did not end in {} s", TIDY_LIMIT.as_secs());
    }
    if tidied.folders + tidied.logs == 0 {
        return;
    }
    let entry = Entry::Tidied {
        folders: tidied.folders,
        logs: tidied.logs,
        bytes: tidied.kib * 1024,
    };
    if let Err(e) = app.graph.record(&entry) {
        crate::log::logln!("loom: tidy: {e}");
    }
}

async fn tidy(app: &App, tidied: &mut Tidied) {
    let repos = match app.repos.list() {
        Ok(repos) => repos,
        Err(e) => return crate::log::logln!("loom: tidy: {e}"),
    };
    let pinned = match app.graph.task_pins() {
        Ok(pins) => pinned(&pins),
        Err(e) => return crate::log::logln!("loom: tidy: {e}"),
    };
    for (repo, dir) in repos {
        if let Err(e) = logs(app, repo, &dir, tidied).await {
            crate::log::logln!("loom: tidy {}: {e}", dir.display());
        }
        if let Err(e) = folders(app, &dir, &pinned, tidied).await {
            crate::log::logln!("loom: tidy {}: {e}", dir.display());
        }
    }
}

fn pinned(pins: &TaskPins) -> HashSet<String> {
    let docs = pins
        .docs
        .iter()
        .filter_map(|id| crate::user_doc::doc_file(id).ok())
        .map(|doc| doc.path.to_string_lossy().into_owned());
    let mut pinned: HashSet<String> = pins.tasks.iter().cloned().collect();
    for text in pins.waits.iter().cloned().chain(docs) {
        pinned.extend(text.split(['/', '\\', '"']).map(str::to_owned));
    }
    pinned
}

async fn logs(app: &App, repo: RepoId, dir: &Path, tidied: &mut Tidied) -> Result<(), String> {
    let text = |e: rusqlite::Error| e.to_string();
    let batches = app.graph.stale_batches(repo).map_err(text)?;
    let deploys = app.graph.deploys().map_err(text)?;
    let open = batches.iter().map(|b| format!("batch-{}", b.id.0)).chain(
        deploys
            .iter()
            .filter(|d| d.repo == repo)
            .map(|d| format!("deploy-{}", d.id)),
    );
    let (n, kib) = swept(app, dir, TIDY_LOGS, open).await?;
    tidied.logs += n;
    tidied.kib += kib;
    Ok(())
}

async fn folders(
    app: &App,
    dir: &Path,
    pinned: &HashSet<String>,
    tidied: &mut Tidied,
) -> Result<(), String> {
    let tasks = app
        .tasks_for(&dir.to_string_lossy())
        .await
        .map_err(|e| format!("{e:?}"))?;
    let unpinned: Vec<String> = tasks
        .of_repo(dir)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|d| !pinned.contains(&d.folder))
        .map(|d| d.shown)
        .collect();
    for part in unpinned.chunks(FOLDERS_PER_RUN) {
        let (n, kib) = swept(app, dir, task::TIDY, part.iter().cloned()).await?;
        tidied.folders += n;
        tidied.kib += kib;
    }
    Ok(())
}

async fn swept(
    app: &App,
    dir: &Path,
    script: &str,
    args: impl Iterator<Item = String>,
) -> Result<(u32, u64), String> {
    let args: Vec<String> = std::iter::once((KEEP_FOR.as_secs() / 60).to_string())
        .chain(args)
        .collect();
    let (code, out) = script_within(app.files(), dir, script, &args, TIDY_LIMIT)
        .await
        .map_err(|e| e.to_string())?;
    let mut words = out.lines().last().unwrap_or_default().split_whitespace();
    match (code, words.next(), words.next()) {
        (0, Some(n), Some(kib)) => match (n.parse(), kib.parse()) {
            (Ok(n), Ok(kib)) => Ok((n, kib)),
            _ => Err(format!("unexpected output: {}", out.trim())),
        },
        _ => Err(format!("exited {code}: {}", out.trim())),
    }
}
