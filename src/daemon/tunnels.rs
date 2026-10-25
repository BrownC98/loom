use std::path::Path;

use super::App;
use crate::tunnel::Launch;
use crate::wsl::{self, LINUX_LOOM};

pub(super) struct WslTunnel(Option<wsl::Tunnel>);

impl WslTunnel {
    pub(super) fn install(shim: &Path, port: u16) -> std::io::Result<WslTunnel> {
        let source = std::env::current_exe()?.with_file_name(LINUX_LOOM);
        let linux = wsl::install_linux(shim, &source)?;
        Ok(WslTunnel(linux.map(|linux| wsl::Tunnel {
            linux,
            sock: wsl::socket(port),
        })))
    }

    pub(super) fn none() -> WslTunnel {
        WslTunnel(None)
    }

    pub(super) fn shim(&self) -> Option<&wsl::Tunnel> {
        self.0.as_ref()
    }
}

impl Launch for WslTunnel {
    fn command(&self, distro: &str) -> Result<tokio::process::Command, String> {
        let tunnel = self
            .0
            .as_ref()
            .ok_or_else(|| format!("데몬 옆에 {LINUX_LOOM}이 없다"))?;
        wsl::prune_linux(&tunnel.linux);
        Ok(wsl::relay(distro, tunnel))
    }

    async fn running(&self, distro: &str) -> bool {
        wsl::running().await.iter().any(|d| d == distro)
    }
}

impl App {
    pub(super) async fn open_tunnel(&self, distro: &str) {
        if let Err(why) = self.tunnels.ensure(distro).await {
            eprintln!("loom: {distro} 터널을 열지 못했다. 훅은 loom.exe로 간다: {why}");
        }
    }
}
