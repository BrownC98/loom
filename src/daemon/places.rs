use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use ts_rs::TS;

use super::{App, Problem, internal};
use crate::profile::LaunchError;
use crate::worktree::{self, Locations, RepoId, RepoView, WslPlace};
use crate::wsl;

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/locations", get(locations))
        .route("/api/distros/{name}", get(distro))
}

/// WSL 배포판 하나의 홈과 기본 워크스페이스 폴더. 둘 다 UNC 경로다.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct DistroView {
    /// 위치 칩을 누르면 폴더 목록이 가는 곳.
    pub home: String,
    /// 설정에 이 배포판의 폴더가 없을 때 쓰는 폴더(`<홈>/loom/workspaces`).
    pub workspace: String,
}

async fn locations() -> Json<Locations> {
    Json(Locations {
        drives: worktree::drives(),
        distros: wsl::distros().await,
    })
}

async fn distro(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
) -> Result<Json<DistroView>, Problem> {
    let home = app.home(&name).await?;
    Ok(Json(DistroView {
        workspace: wsl_workspace(&home)?.to_string_lossy().into_owned(),
        home: home.to_string_lossy().into_owned(),
    }))
}

fn wsl_workspace(home: &FsPath) -> Result<PathBuf, Problem> {
    worktree::workspace_in(&home.join("loom")).map_err(internal)
}

impl App {
    async fn home(&self, distro: &str) -> Result<PathBuf, Problem> {
        let home = self
            .homes
            .get(distro)
            .await
            .map_err(|e| Problem::Launch(LaunchError::Wsl(e)))?;
        Ok(PathBuf::from(wsl::windows_path(distro, &home)))
    }

    pub(super) async fn repo_view(&self, id: RepoId, path: &FsPath) -> RepoView {
        let wsl = match wsl::of(path) {
            Some(at) => Some(WslPlace {
                path: match self.homes.get(&at.distro).await {
                    Ok(home) => wsl::tilde(&at.path, &home),
                    Err(_) => at.path,
                },
                distro: at.distro,
            }),
            None => None,
        };
        worktree::view(id, path, wsl).await
    }

    pub(super) async fn workspace(&self, repo: &FsPath) -> Result<PathBuf, Problem> {
        let settings = self.settings.get().map_err(internal)?;
        let Some(at) = wsl::of(repo) else {
            return match settings.workspace_dir {
                Some(dir) => Ok(PathBuf::from(dir)),
                None => worktree::default_workspace().map_err(internal),
            };
        };
        match settings.wsl_workspace_dirs.get(&at.distro) {
            Some(dir) => Ok(PathBuf::from(dir)),
            None => wsl_workspace(&self.home(&at.distro).await?),
        }
    }
}
