use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use ts_rs::TS;

use super::runtime::unconnected;
use super::{App, Problem, internal};
use crate::distro::Host;
use crate::place::{self, Machine};
use crate::protocol::LaunchError;
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
        let wsl_failed = |e| Problem::Launch(LaunchError::Wsl(e));
        let host = self
            .host(Host::Distro(distro.to_owned()))
            .map_err(wsl_failed)?;
        let home = wsl::home(&host).await.map_err(wsl_failed)?;
        Ok(PathBuf::from(wsl::windows_path(distro, &home)))
    }

    async fn home_inside(&self, distro: &str) -> Result<String, crate::protocol::InsideError> {
        wsl::home(&self.host(Host::Distro(distro.to_owned()))?).await
    }

    pub(super) async fn repo_view(&self, id: RepoId, path: &FsPath) -> RepoView {
        let wsl = match place::of(path) {
            Machine::Wsl(at) => Some(WslPlace {
                path: match self.home_inside(&at.distro).await {
                    Ok(home) => wsl::tilde(&at.path, &home),
                    Err(_) => at.path,
                },
                distro: at.distro,
            }),
            Machine::Pc | Machine::Ssh(_) => None,
        };
        worktree::view(id, path, wsl).await
    }

    pub(super) async fn workspace(&self, repo: &FsPath) -> Result<PathBuf, Problem> {
        let settings = self.settings.get().map_err(internal)?;
        match place::of(repo) {
            Machine::Pc => match settings.workspace_dir {
                Some(dir) => Ok(PathBuf::from(dir)),
                None => worktree::default_workspace().map_err(internal),
            },
            Machine::Wsl(at) => match settings.wsl_workspace_dirs.get(&at.distro) {
                Some(dir) => Ok(PathBuf::from(dir)),
                None => wsl_workspace(&self.home(&at.distro).await?),
            },
            Machine::Ssh(at) => Err(Problem::Launch(LaunchError::Wsl(unconnected(&at)))),
        }
    }
}
