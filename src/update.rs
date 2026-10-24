use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::launch;

const RELEASES: Option<&str> = option_env!("LOOM_RELEASES");
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const CONNECT_FOR: Duration = Duration::from_secs(10);
const RESPONSE_FOR: Duration = Duration::from_secs(30);
const APP_NAME: &str = "loom-app";
const APP_UPDATE_FLAG: &str = "--update";

/// 화면이 보는 업데이트 상태. 바뀌면 `ViewChanged`를 보낸다. 받는 동안의 퍼센트는 보내지 않는다: 화면이 다시 읽는다.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateView {
    /// 이 데몬이 확인하고 바꿀 수 있다. 개발 빌드와 받을 파일이 없는 OS는 거짓이다.
    pub checks: bool,
    /// 이 데몬보다 새 판. 없으면 최신이다(또는 아직 확인하지 않았다).
    pub latest: Option<String>,
    pub step: Option<UpdateStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateStep {
    Available,
    Downloading { percent: u8 },
    Restarting,
    Failed { message: String },
}

#[derive(Debug, PartialEq)]
enum Install {
    Standalone { exe: PathBuf, asset: &'static str },
    App { app: PathBuf },
    Unsupported,
}

impl Install {
    fn of(exe: &Path, asset: Option<&'static str>) -> Self {
        let app = exe.with_file_name(format!("{APP_NAME}{}", std::env::consts::EXE_SUFFIX));
        match asset {
            _ if app.is_file() => Install::App { app },
            Some(asset) => Install::Standalone {
                exe: exe.to_owned(),
                asset,
            },
            None => Install::Unsupported,
        }
    }
}

fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("loom-web-linux-x64"),
        ("windows", "x86_64") => Some("loom-web-windows-x64.exe"),
        ("macos", "aarch64") => Some("loom-web-macos-arm64"),
        _ => None,
    }
}

fn latest_url(releases: &str) -> String {
    format!("{releases}/latest/download/latest.json")
}

fn asset_url(releases: &str, version: &str, asset: &str) -> String {
    format!("{releases}/download/v{version}/{asset}")
}

#[derive(Deserialize)]
struct Latest {
    version: String,
}

#[derive(Default)]
struct State {
    latest: Option<String>,
    step: Option<UpdateStep>,
}

pub struct Updater {
    releases: Option<&'static str>,
    install: Install,
    listen: SocketAddr,
    state: Mutex<State>,
    notify: Box<dyn Fn() + Send + Sync>,
}

impl Updater {
    pub fn new(listen: SocketAddr, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let install = match std::env::current_exe() {
            Ok(exe) => {
                remove_aside(&exe);
                Install::of(
                    &exe,
                    asset_name(std::env::consts::OS, std::env::consts::ARCH),
                )
            }
            Err(_) => Install::Unsupported,
        };
        Self {
            releases: RELEASES,
            install,
            listen,
            state: Mutex::default(),
            notify: Box::new(notify),
        }
    }

    fn checks(&self) -> Option<&'static str> {
        self.releases
            .filter(|_| !matches!(self.install, Install::Unsupported))
    }

    pub fn view(&self) -> UpdateView {
        let state = self.state.lock().unwrap();
        UpdateView {
            checks: self.checks().is_some(),
            latest: state.latest.clone(),
            step: state.step.clone(),
        }
    }

    pub async fn run(self: Arc<Self>) {
        if self.checks().is_none() {
            return;
        }
        loop {
            let _ = self.clone().check().await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    }

    pub async fn check(self: Arc<Self>) -> Result<UpdateView, String> {
        let Some(releases) = self.checks() else {
            return Err("this loom build does not check for updates".into());
        };
        let latest = tokio::task::spawn_blocking(move || fetch_latest(releases))
            .await
            .map_err(|e| e.to_string())??;
        let newer = launch::newer(&latest, crate::protocol::VERSION).then_some(latest);
        self.change(|state| {
            if matches!(state.step, Some(UpdateStep::Downloading { .. })) {
                return;
            }
            state.step = newer.as_ref().map(|_| UpdateStep::Available);
            state.latest = newer;
        });
        Ok(self.view())
    }

    pub fn install(self: Arc<Self>) -> Result<UpdateView, String> {
        let mut version = None;
        self.change(|state| {
            if matches!(
                state.step,
                Some(UpdateStep::Available | UpdateStep::Failed { .. })
            ) {
                version = state.latest.clone();
                state.step = Some(UpdateStep::Downloading { percent: 0 });
            }
        });
        let Some(version) = version else {
            return Ok(self.view());
        };
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let step = match me.replace(&version) {
                Ok(()) => UpdateStep::Restarting,
                Err(message) => UpdateStep::Failed { message },
            };
            me.change(|state| state.step = Some(step));
        });
        Ok(self.view())
    }

    fn replace(&self, version: &str) -> Result<(), String> {
        let releases = self.checks().ok_or("updates are off")?;
        match &self.install {
            Install::Standalone { exe, asset } => {
                let new = exe.with_file_name(format!(
                    "{}.new",
                    exe.file_name().unwrap_or_default().to_string_lossy()
                ));
                download(&asset_url(releases, version, asset), &new, |percent| {
                    self.state.lock().unwrap().step = Some(UpdateStep::Downloading { percent });
                })?;
                swap(exe, &new).map_err(|e| format!("{}: {e}", exe.display()))?;
                launch::restart_with(exe, self.listen)
            }
            Install::App { app } => launch::spawn_detached(app, &[APP_UPDATE_FLAG]).map(drop),
            Install::Unsupported => Err("no update for this system".into()),
        }
    }

    fn change(&self, edit: impl FnOnce(&mut State)) {
        let before = self.view();
        edit(&mut self.state.lock().unwrap());
        if self.view() != before {
            (self.notify)();
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_FOR))
        .timeout_recv_response(Some(RESPONSE_FOR))
        .build()
        .into()
}

fn fetch_latest(releases: &str) -> Result<String, String> {
    let url = latest_url(releases);
    let latest: Latest = agent()
        .get(&url)
        .call()
        .and_then(|mut r| r.body_mut().read_json())
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(latest.version)
}

fn download(url: &str, to: &Path, mut progress: impl FnMut(u8)) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("{url}: {e}");
    let mut response = agent().get(url).call().map_err(|e| fail(&e))?;
    let total = response.body().content_length().unwrap_or(0);
    let mut body = response.body_mut().as_reader();
    let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut buf = vec![0; 64 * 1024];
    let (mut done, mut shown) = (0u64, 0u8);
    loop {
        let n = body.read(&mut buf).map_err(|e| fail(&e))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])
            .map_err(|e| format!("{}: {e}", to.display()))?;
        done += n as u64;
        if let Some(percent) = (done * 100).checked_div(total) {
            let percent = percent.min(100) as u8;
            if percent != shown {
                shown = percent;
                progress(percent);
            }
        }
    }
    file.sync_all()
        .map_err(|e| format!("{}: {e}", to.display()))
}

#[cfg(not(windows))]
fn swap(exe: &Path, new: &Path) -> std::io::Result<()> {
    swap_over(exe, new)
}

#[cfg(windows)]
fn swap(exe: &Path, new: &Path) -> std::io::Result<()> {
    swap_aside(exe, new, now_secs())
}

#[cfg(any(not(windows), test))]
fn swap_over(exe: &Path, new: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(new, exe)
}

#[cfg(any(windows, test))]
fn swap_aside(exe: &Path, new: &Path, stamp: u64) -> std::io::Result<()> {
    let aside = exe.with_file_name(format!(
        "{}.{stamp}.old",
        exe.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::rename(exe, &aside)?;
    std::fs::rename(new, exe).inspect_err(|_| {
        let _ = std::fs::rename(&aside, exe);
    })
}

#[cfg(windows)]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn remove_aside(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file.starts_with(&prefix) && file.ends_with(".old") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_match_release_files() {
        assert_eq!(asset_name("linux", "x86_64"), Some("loom-web-linux-x64"));
        assert_eq!(
            asset_name("windows", "x86_64"),
            Some("loom-web-windows-x64.exe")
        );
        assert_eq!(asset_name("macos", "aarch64"), Some("loom-web-macos-arm64"));
        assert_eq!(asset_name("linux", "aarch64"), None);
        assert_eq!(asset_name("macos", "x86_64"), None);
        let workflow = include_str!("../.github/workflows/release.yml");
        for asset in [
            "loom-web-linux-x64",
            "loom-web-windows-x64.exe",
            "loom-web-macos-arm64",
        ] {
            assert!(workflow.contains(asset), "{asset} is not uploaded");
        }
    }

    #[test]
    fn urls_follow_release_layout() {
        let base = "https://github.com/BrownC98/loom/releases";
        assert_eq!(
            latest_url(base),
            "https://github.com/BrownC98/loom/releases/latest/download/latest.json"
        );
        assert_eq!(
            asset_url(base, "0.2.5", "loom-web-linux-x64"),
            "https://github.com/BrownC98/loom/releases/download/v0.2.5/loom-web-linux-x64"
        );
    }

    #[test]
    fn checks_every_six_hours() {
        assert_eq!(CHECK_EVERY, Duration::from_secs(21_600));
    }

    #[test]
    fn install_is_found_by_the_files_next_to_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("loom");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(
            Install::of(&exe, Some("loom-web-linux-x64")),
            Install::Standalone {
                exe: exe.clone(),
                asset: "loom-web-linux-x64"
            }
        );
        assert_eq!(Install::of(&exe, None), Install::Unsupported);
        let app = dir
            .path()
            .join(format!("loom-app{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&app, "").unwrap();
        assert_eq!(
            Install::of(&exe, Some("loom-web-macos-arm64")),
            Install::App { app: app.clone() }
        );
        assert_eq!(Install::of(&exe, None), Install::App { app });
    }

    #[test]
    fn aside_swap_keeps_the_old_file_and_cleans_it_later() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("loom.exe");
        let new = dir.path().join("loom.exe.new");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        swap_aside(&exe, &new, 42).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        let aside = dir.path().join("loom.exe.42.old");
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "old");
        assert!(!new.exists());

        std::fs::write(&exe, "running").unwrap();
        assert!(swap_aside(&exe, &new, 43).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "running");
        assert!(!dir.path().join("loom.exe.43.old").exists());

        std::fs::write(dir.path().join("other.exe.1.old"), "").unwrap();
        remove_aside(&exe);
        assert!(!aside.exists());
        assert!(exe.exists() && dir.path().join("other.exe.1.old").exists());
    }

    #[test]
    fn over_swap_replaces_and_makes_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("loom");
        let new = dir.path().join("loom.new");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        swap_over(&exe, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[tokio::test]
    async fn install_needs_a_newer_version_first() {
        let updater = Arc::new(Updater {
            releases: Some("http://127.0.0.1:9"),
            install: Install::Unsupported,
            listen: launch::DEFAULT_LISTEN,
            state: Mutex::default(),
            notify: Box::new(|| {}),
        });
        let view = updater.clone().install().unwrap();
        assert_eq!(view.step, None);
        assert!(!view.checks);
    }
}
