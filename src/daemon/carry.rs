//! 옮기기: 옛 WSL 데몬(`~/.loom`)의 저장소·노드·설정·질문·폰을 이 Windows 데몬으로 옮기고, 에이전트 노드를 같은 세션으로 다시 띄운다.
//! 2026-10-22 사람 결정(Q375 A): 바꾸는 동안 살아 있는 노드의 일을 잃지 않는다. 근거: docs/design.md 「데몬 하나」.

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

/// 다시 띄운 노드의 첫 입력. 턴 중에 옮겨진 노드는 이 입력으로 일을 잇는다.
const NOTICE: &str = "loom moved this node from the WSL daemon to the Windows daemon and started your process again with the same conversation. Background commands that you started stopped. If a task was in progress, continue it.";

/// `loom carry <옛 데이터 폴더>`. `from`은 옛 데몬의 데이터 폴더(`~/.loom`)의 UNC 경로다.
#[derive(Deserialize)]
pub struct CarryRequest {
    pub from: String,
}

/// 옮긴 결과. `ended`는 다시 띄우지 못한 노드와 그 까닭이다.
#[derive(Serialize)]
pub struct Carried {
    pub relaunched: Vec<String>,
    pub ended: Vec<String>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/carry", post(carry))
}

/// 부른 쪽(`host.sh`를 돈 노드)은 옛 노드를 끝낼 때 함께 끝난다. 끊겨도 끝까지 한다.
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
        // 옛 파일은 읽기만 한다. 복사본에서 스키마를 올리고 경로를 바꾼다.
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

/// 옛 데몬이 적은 Linux 경로를 이 데몬이 그 배포판 폴더를 읽는 UNC 경로로 바꾼다.
/// 옛 데몬에서 「이 PC」였던 설정(워크스페이스 폴더, 사용 중 계정)은 그 배포판 칸으로 옮긴다.
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
