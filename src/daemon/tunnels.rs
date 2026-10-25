use std::path::Path;
use std::sync::Arc;

use super::App;
use crate::graph::Graph;
use crate::tunnel::Launch;
use crate::wsl::{self, LINUX_LOOM};

pub(super) struct WslTunnel {
    shim: Option<wsl::Tunnel>,
    graph: Arc<Graph>,
}

impl WslTunnel {
    pub(super) fn install(shim: &Path, port: u16, graph: Arc<Graph>) -> std::io::Result<WslTunnel> {
        let source = std::env::current_exe()?.with_file_name(LINUX_LOOM);
        let linux = wsl::install_linux(shim, &source)?;
        Ok(WslTunnel {
            shim: linux.map(|linux| wsl::Tunnel {
                linux,
                sock: wsl::socket(port),
            }),
            graph,
        })
    }

    pub(super) fn none(graph: Arc<Graph>) -> WslTunnel {
        WslTunnel { shim: None, graph }
    }

    pub(super) fn shim(&self) -> Option<&wsl::Tunnel> {
        self.shim.as_ref()
    }
}

impl Launch for WslTunnel {
    fn command(&self, distro: &str) -> Result<tokio::process::Command, String> {
        let tunnel = self
            .shim
            .as_ref()
            .ok_or_else(|| format!("데몬 옆에 {LINUX_LOOM}이 없다"))?;
        wsl::prune_linux(&tunnel.linux);
        Ok(wsl::relay(distro, tunnel))
    }

    async fn needed(&self, distro: &str) -> bool {
        let nodes = match self.graph.active() {
            Ok(nodes) => nodes,
            Err(e) => {
                eprintln!("loom: {distro} 터널에 노드가 있는지 보지 못했다: {e}");
                return true;
            }
        };
        nodes.iter().any(|node| {
            self.graph.place_of(node.id).is_ok_and(|place| {
                wsl::of(Path::new(&place.worktree)).is_some_and(|at| at.distro == distro)
            })
        })
    }
}

impl App {
    pub(super) async fn open_tunnel(&self, distro: &str) {
        if let Err(why) = self.tunnels.ensure(distro).await {
            eprintln!("loom: {distro} 터널을 열지 못했다. 훅은 loom.exe로 간다: {why}");
        }
    }
}

pub(super) struct Recheck {
    app: Arc<App>,
    distro: Option<String>,
}

impl Recheck {
    pub(super) fn after(app: &Arc<App>, distro: Option<String>) -> Recheck {
        Recheck {
            app: app.clone(),
            distro,
        }
    }
}

impl Drop for Recheck {
    fn drop(&mut self) {
        if let Some(distro) = &self.distro {
            self.app.tunnels.recheck(distro);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::graph::{Launched, Place};
    use crate::protocol::AgentId;
    use crate::worktree::RepoId;

    #[tokio::test]
    async fn a_distro_needs_its_tunnel_only_while_a_node_runs_there() {
        let dir = tempfile::tempdir().unwrap();
        let database = crate::db::open(&dir.path().join("loom.db")).unwrap();
        let repo = r"\\wsl.localhost\Ubuntu\repo";
        database
            .lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, ?1)", [repo])
            .unwrap();
        let graph = Arc::new(Graph::open(database, crate::profile::linked).unwrap());
        let launch = WslTunnel::none(graph.clone());
        assert!(!launch.needed("Ubuntu").await);
        let place = Place {
            repo: RepoId(1),
            worktree: repo.into(),
        };
        let launched = Launched {
            token: "t".into(),
            server: None,
            account: None,
            rules: None,
        };
        let node = graph
            .add_root(
                &place,
                Some(AgentId("claude".into())),
                None,
                &launched,
                |_| Ok(()),
            )
            .unwrap();
        assert!(launch.needed("Ubuntu").await);
        assert!(!launch.needed("Debian").await);
        graph.exited(node, 0).unwrap();
        assert!(!launch.needed("Ubuntu").await);
    }
}
