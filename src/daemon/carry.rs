use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{App, Problem, internal};
use crate::db;
use crate::graph::{Entry, Who};
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
        let distro = self.distro(&at.distro).map_err(|e| e.to_string())?;
        let copy = std::env::temp_dir().join(format!("loom-carry-{}", std::process::id()));
        std::fs::create_dir_all(&copy).map_err(|e| e.to_string())?;
        let copied = self.carry_from_copy(Path::new(from), &copy, &at);
        let _ = std::fs::remove_dir_all(&copy);
        copied?;
        self.graph.publish_view_changed();
        let active = self.graph.active().map_err(|e| e.to_string())?;
        for node in &active {
            if let Err(e) = self.end_detached(node).await {
                eprintln!("loom: 노드 {}의 옛 세션을 끝내지 못했다: {e}", node.id.0);
            }
        }
        let mut carried = Carried {
            relaunched: Vec::new(),
            ended: Vec::new(),
        };
        if let Err(e) = wsl::end_hosts(&distro, &home).await {
            for node in active {
                let _ = self.graph.lost(node.id);
                carried
                    .ended
                    .push(format!("{}: 옛 프로세스가 끝나지 않았다: {e}", node.name));
            }
            return Ok(carried);
        }
        for node in active {
            let who = Who {
                id: Some(node.id),
                name: node.name.clone(),
            };
            let revived = self.revive(node, NOTICE).await.map_err(|e| e.to_string());
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
        Settings::new(old, crate::profile::check_choice)
            .move_into_wsl(&at.distro)
            .map_err(|e| e.to_string())?;
        db::import(&self.database, &file, |linux| at.unc(linux))
    }
}
