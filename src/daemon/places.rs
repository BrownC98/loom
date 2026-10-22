//! 실행 위치(이 PC, WSL 배포판)의 라우트와 위치마다 다른 값: 위치 칩, 배포판 홈, 저장소의 배포판 표시, 워크스페이스 폴더.
//! 위치는 저장소 경로에서 나온다(`wsl::of`). 따로 저장하지 않는다. 근거: docs/design.md 「데몬 하나」, 목업 docs/mockups/wsl-place.html.

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

/// 배포판을 켜지 않는다. 저장소 추가 창이 열릴 때마다 부른다.
async fn locations() -> Json<Locations> {
    Json(Locations {
        drives: worktree::drives(),
        distros: wsl::distros().await,
    })
}

/// 꺼진 배포판은 켠다. 화면은 오래 걸리면 「시작하는 중」을 보인다(목업 ⑥b).
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

/// `<홈>/loom/workspaces`. 없으면 만든다.
fn wsl_workspace(home: &FsPath) -> Result<PathBuf, Problem> {
    worktree::workspace_in(&home.join("loom")).map_err(internal)
}

impl App {
    /// 배포판 홈의 UNC 경로.
    async fn home(&self, distro: &str) -> Result<PathBuf, Problem> {
        let home = self
            .homes
            .get(distro)
            .await
            .map_err(|e| Problem::Launch(LaunchError::Wsl(e)))?;
        Ok(PathBuf::from(wsl::windows_path(distro, &home)))
    }

    /// 화면에 보내는 저장소. WSL 저장소면 배포판과 `~/…` 경로를 붙인다.
    /// 저장소의 git도 그 배포판에서 돈다. 홈을 읽는 `wsl.exe`는 git이 켤 배포판을 켤 뿐이고, 한 번 읽으면 다시 부르지 않는다.
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

    /// `repo`의 새 워크트리를 만드는 폴더. 저장소와 같은 위치의 설정 폴더, 없으면 그 위치의 기본값이다.
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
