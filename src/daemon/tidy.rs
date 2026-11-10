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
const EDGES: &[char] = &[
    '(', ')', '[', ']', '<', '>', '`', '\'', '.', ',', ';', ':', ' ',
];

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
        Ok(pins) => pins_of(&pins),
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

fn pins_of(pins: &TaskPins) -> HashSet<String> {
    let docs = pins
        .docs
        .iter()
        .filter_map(|id| crate::user_doc::doc_file(id).ok())
        .map(|doc| doc.path.to_string_lossy().into_owned());
    let mut pinned: HashSet<String> = pins.tasks.iter().cloned().collect();
    for text in pins.waits.iter().cloned().chain(docs) {
        let decoded = percent_encoding::percent_decode_str(&text).decode_utf8_lossy();
        for text in [text.as_str(), &decoded] {
            for piece in text.split(['/', '\\', '"']) {
                pinned.insert(piece.to_owned());
                pinned.insert(piece.trim_matches(EDGES).to_owned());
            }
        }
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
    let args: Vec<String> = std::iter::once(minutes()).chain(open).collect();
    let (n, kib) = counted(&bash(app, dir, TIDY_LOGS, &args).await?)?;
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
    let data = crate::db::data_dir().map_err(|e| e.to_string())?;
    let me = data.to_string_lossy();
    if tasks.owner().map_err(|e| e.to_string())?.as_deref() != Some(&*me) {
        return Ok(());
    }
    let unpinned: Vec<String> = tasks
        .of_repo(dir)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|d| d.folder)
        .filter(|name| !pinned.contains(name) && !name.contains('\n'))
        .collect();
    let root = tasks.shown().to_owned();
    let parts: Vec<&[String]> = match unpinned.is_empty() {
        true => vec![&[]],
        false => unpinned.chunks(FOLDERS_PER_RUN).collect(),
    };
    for part in parts {
        let args: Vec<String> = [minutes(), root.clone()]
            .into_iter()
            .chain(part.iter().cloned())
            .collect();
        let hidden = bash(app, dir, task::HIDE, &args).await?;
        let pinned = app.graph.task_pins().map(|p| pins_of(&p));
        let (keep, drop): (Vec<&str>, Vec<&str>) = hidden
            .lines()
            .filter(|name| !name.is_empty())
            .partition(|name| pinned.as_ref().map_or(true, |p| p.contains(*name)));
        let args: Vec<String> = std::iter::once(root.as_str())
            .chain(keep)
            .chain(["--"])
            .chain(drop)
            .map(str::to_owned)
            .collect();
        let (n, kib) = counted(&bash(app, dir, task::DROP, &args).await?)?;
        tidied.folders += n;
        tidied.kib += kib;
    }
    Ok(())
}

fn minutes() -> String {
    (KEEP_FOR.as_secs() / 60).to_string()
}

async fn bash(app: &App, dir: &Path, script: &str, args: &[String]) -> Result<String, String> {
    match script_within(app.files(), dir, script, args, TIDY_LIMIT)
        .await
        .map_err(|e| e.to_string())?
    {
        (0, out) => Ok(out),
        (code, out) => Err(format!("exited {code}: {}", out.trim())),
    }
}

fn counted(out: &str) -> Result<(u32, u64), String> {
    let mut words = out.lines().last().unwrap_or_default().split_whitespace();
    match (words.next().map(str::parse), words.next().map(str::parse)) {
        (Some(Ok(n)), Some(Ok(kib))) => Ok((n, kib)),
        _ => Err(format!("unexpected output: {}", out.trim())),
    }
}
