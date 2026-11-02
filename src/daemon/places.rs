use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use api::settings::places::{DistroView, ReachView, SystemView};
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};

use super::ssh_hosts::Reach;
use super::{App, internal};
use crate::distro::Host;
use crate::place::{self, Machine};
use crate::ssh::{Failure, SSH_DIR, System};
use crate::worktree;
use crate::wsl;
use api::graph::errors::{LaunchError, Problem};
use api::ids::RepoId;
use api::repos::{Locations, RepoView};

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/locations", get(locations))
        .route("/api/distros/{name}", get(distro))
        .route("/api/ssh/{host}", get(ssh_reach).post(ssh_connect))
        .route("/api/ssh/{host}/reconnect", post(ssh_reconnect))
        .route("/api/ssh/{host}/release", post(ssh_release))
}

fn reach_view(reach: &Reach) -> ReachView {
    match reach {
        Reach::Reached(remote) => ReachView::Reached {
            home: remote.home.clone(),
            system: match &remote.system {
                System::Linux => SystemView::Linux,
                System::Mac => SystemView::Mac,
                System::Windows { distro } => SystemView::Windows {
                    distro: distro.clone(),
                },
            },
        },
        Reach::Unreached(Failure::UnknownHostKey { folder, .. }) => ReachView::UnknownHostKey {
            folder: (*folder).to_owned(),
        },
        Reach::Unreached(Failure::AuthFailed) => ReachView::AuthFailed {
            folder: SSH_DIR.to_owned(),
        },
        Reach::Unreached(Failure::Unreachable(cause)) => ReachView::Unreachable {
            cause: cause.clone(),
        },
        Reach::Unreached(Failure::UnsupportedMac { cpu }) => {
            ReachView::UnsupportedMac { cpu: cpu.clone() }
        }
        Reach::Unreached(Failure::VersionMismatch) => ReachView::VersionMismatch,
        Reach::Unreached(Failure::Failed(cause)) => ReachView::Failed {
            cause: cause.clone(),
        },
    }
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

async fn ssh_reach(
    State(app): State<Arc<App>>,
    Path(host): Path<String>,
) -> Json<Option<ReachView>> {
    Json(app.ssh_reach(&host).as_ref().map(reach_view))
}

async fn ssh_connect(
    State(app): State<Arc<App>>,
    Path(host): Path<String>,
) -> Result<Json<ReachView>, Problem> {
    let _use = app.use_ssh(&host).await;
    let distros = app.distros.clone();
    let remote = tokio::task::spawn_blocking(move || distros.remote(&Host::Ssh(host)))
        .await
        .map_err(internal)?;
    let reach = match remote {
        Ok(remote) => Reach::Reached(remote),
        Err(failure) => Reach::Unreached(failure),
    };
    Ok(Json(reach_view(&reach)))
}

async fn ssh_reconnect(State(app): State<Arc<App>>, Path(host): Path<String>) -> Json<()> {
    app.reconnect_ssh(&host);
    Json(())
}

async fn ssh_release(State(app): State<Arc<App>>, Path(host): Path<String>) -> Json<()> {
    app.release_ssh(&host).await;
    Json(())
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

    async fn home_inside(&self, distro: &str) -> Result<String, api::graph::errors::InsideError> {
        wsl::home(&self.host(Host::Distro(distro.to_owned()))?).await
    }

    pub(super) async fn repo_view(&self, id: RepoId, path: &FsPath) -> RepoView {
        let machine = place::of(path);
        let shown = match &machine {
            Machine::Pc => path.to_string_lossy().into_owned(),
            Machine::Wsl(at) => match self.home_inside(&at.distro).await {
                Ok(home) => wsl::tilde(&at.path, &home),
                Err(_) => at.path.clone(),
            },
            Machine::Ssh(at) => match self.ssh_home(&at.host) {
                Some(home) => wsl::tilde(&at.path, &home),
                None => at.path.clone(),
            },
        };
        let worktrees = match worktree::listed(self.files(), path).await {
            Ok(listed) => {
                let mut lists = self.worktree_lists.lock().unwrap();
                lists.insert(path.to_owned(), listed.clone());
                listed
            }
            Err(_) => {
                let lists = self.worktree_lists.lock().unwrap();
                lists
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| worktree::alone(path))
            }
        };
        worktree::view(
            self.files(),
            id,
            path,
            worktree::place_view(&machine),
            shown,
            worktrees,
        )
        .await
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
            Machine::Ssh(at) => {
                let distros = self.distros.clone();
                let host = Host::Ssh(at.host.clone());
                let remote = tokio::task::spawn_blocking(move || distros.remote(&host))
                    .await
                    .map_err(internal)?
                    .map_err(|e| {
                        Problem::Launch(LaunchError::Wsl(api::graph::errors::InsideError::Start {
                            distro: at.host.clone(),
                            cause: e.to_string(),
                        }))
                    })?;
                let home = remote.home.trim_end_matches('/');
                let dir = PathBuf::from(at.url(&worktree::workspace_under(home)));
                self.files().create_dir_all(&dir).map_err(internal)?;
                Ok(dir)
            }
        }
    }
}
