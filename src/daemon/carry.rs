use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{App, Problem, internal};
use crate::db;
use crate::graph::{Entry, Who};
use crate::profile::ServerLink;
use crate::settings::Settings;
use crate::wsl::{self, WslPath};

const NOTICE: &str = "loom moved this node from the WSL daemon to the Windows daemon and started your process again with the same conversation. Background commands that you started stopped. If a task was in progress, continue it.";

#[derive(Deserialize)]
pub struct CarryRequest {
    pub from: String,
}

#[derive(Serialize)]
pub struct Carried {
    pub relaunched: Vec<String>,
    pub ended: Vec<String>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/carry", post(carry))
}

async fn carry(
    State(app): State<Arc<App>>,
    Json(req): Json<CarryRequest>,
) -> Result<Json<Carried>, Problem> {
    let result = tokio::spawn(async move { app.carry(&req.from).await })
        .await
        .map_err(internal)?;
    if let Err(e) = &result {
        eprintln!("loom: 옮기지 못했다: {e}");
    }
    result.map(Json).map_err(Problem::Internal)
}

impl App {
    async fn carry(self: &Arc<Self>, from: &str) -> Result<Carried, String> {
        let at = wsl::location(from).ok_or_else(|| format!("{from} is not a WSL folder"))?;
        let home = at
            .path
            .rsplit_once('/')
            .map(|(home, _)| home.to_owned())
            .ok_or_else(|| format!("{from} has no parent folder"))?;
        let copy = std::env::temp_dir().join(format!("loom-carry-{}", std::process::id()));
        std::fs::create_dir_all(&copy).map_err(|e| e.to_string())?;
        let copied = self.carry_from_copy(Path::new(from), &copy, &at);
        let _ = std::fs::remove_dir_all(&copy);
        copied?;
        self.graph.publish_view_changed();
        let active = self.graph.active().map_err(|e| e.to_string())?;
        let servers: Vec<u32> = active
            .iter()
            .filter_map(|n| match &n.server {
                Some(ServerLink::Codex { pid, .. } | ServerLink::OpenCode2 { pid, .. }) => *pid,
                None => None,
            })
            .collect();
        wsl::end_hosts(&at.distro, &home, &servers).await?;
        let mut carried = Carried {
            relaunched: Vec::new(),
            ended: Vec::new(),
        };
        for node in active {
            let who = Who {
                id: Some(node.id),
                name: node.name.clone(),
            };
            let revived = self.revive(node, NOTICE).await;
            let _ = self.graph.record(&Entry::Relaunched {
                node: who.clone(),
                switched: None,
                error: revived.as_ref().err().cloned(),
            });
            match revived {
                Ok(()) => carried.relaunched.push(who.name),
                Err(e) => {
                    if let Some(id) = who.id {
                        let _ = self.graph.lost(id);
                    }
                    carried.ended.push(format!("{}: {e}", who.name));
                }
            }
        }
        Ok(carried)
    }

    fn carry_from_copy(&self, from: &Path, copy: &Path, at: &WslPath) -> Result<(), String> {
        let file = copy.join("loom.db");
        for name in ["loom.db", "loom.db-wal"] {
            let source = from.join(name);
            if source.exists() {
                std::fs::copy(&source, copy.join(name))
                    .map_err(|e| format!("{}: {e}", source.display()))?;
            }
        }
        let old = db::open(&file).map_err(|e| e.to_string())?;
        to_windows(&old, at).map_err(|e| e.to_string())?;
        drop(old);
        db::import(&self.database, &file)
    }
}

fn to_windows(old: &db::Database, at: &WslPath) -> rusqlite::Result<()> {
    {
        let conn = old.lock();
        for (table, column) in [("repos", "path"), ("nodes", "worktree")] {
            let rows: Vec<(i64, String)> = conn
                .prepare(&format!(
                    "SELECT rowid, {column} FROM {table} WHERE {column} LIKE '/%' AND {column} NOT LIKE '//%'"
                ))?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (rowid, path) in rows {
                conn.execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE rowid = ?2"),
                    (at.unc(&path), rowid),
                )?;
            }
        }
        let spots: Vec<(String, String)> = conn
            .prepare(
                "SELECT spot, spot ->> 'path' FROM spots WHERE spot ->> 'kind' = 'worktree' AND spot ->> 'path' LIKE '/%'",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (spot, path) in spots {
            conn.execute(
                "UPDATE spots SET spot = json_set(spot, '$.path', ?1) WHERE spot = ?2",
                (at.unc(&path), spot),
            )?;
        }
    }
    let distro = at.distro.clone();
    Settings::new(old.clone()).update(|view| {
        if let Some(dir) = view.workspace_dir.take_if(|d| d.starts_with('/')) {
            view.wsl_workspace_dirs.entry(distro.clone()).or_insert(dir);
        }
        let in_use = view.wsl_accounts.entry(distro).or_default();
        in_use.claude = in_use.claude.take().or(view.claude_account.take());
        in_use.codex = in_use.codex.take().or(view.codex_account.take());
    })
}
