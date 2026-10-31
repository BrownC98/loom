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

/// 화면이 보는 업데이트 상태. 받는 동안의 퍼센트를 포함해 바뀔 때마다 `ViewChanged`를 보낸다.
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
    Standalone {
        exe: PathBuf,
        asset: &'static str,
        besides: &'static [Beside],
    },
    App {
        app: PathBuf,
    },
    Unsupported,
}

#[derive(Debug, PartialEq)]
struct Beside {
    asset: &'static str,
    file: &'static str,
}

const WINDOWS_BESIDES: &[Beside] = &[Beside {
    asset: "loom-web-linux-x64",
    file: crate::distro::LINUX_LOOM,
}];
const MAC_ASSET: &str = "loom-web-macos-arm64";

impl Install {
    fn of(exe: &Path, asset: Option<&'static str>, besides: &'static [Beside]) -> Self {
        let app = exe.with_file_name(format!("{APP_NAME}{}", std::env::consts::EXE_SUFFIX));
        match asset {
            _ if app.is_file() => Install::App { app },
            Some(asset) => Install::Standalone {
                exe: exe.to_owned(),
                asset,
                besides,
            },
            None => Install::Unsupported,
        }
    }
}

fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("loom-web-linux-x64"),
        ("windows", "x86_64") => Some("loom-web-windows-x64.exe"),
        ("macos", "aarch64") => Some(MAC_ASSET),
        _ => None,
    }
}

fn besides(os: &str, arch: &str) -> &'static [Beside] {
    match (os, arch) {
        ("windows", "x86_64") => WINDOWS_BESIDES,
        _ => &[],
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
                let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
                Install::of(&exe, asset_name(os, arch), besides(os, arch))
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
            Install::Standalone {
                exe,
                asset,
                besides,
            } => {
                let besides = fetch_besides(releases, version, exe, besides)?;
                let new = staged(exe);
                download(&asset_url(releases, version, asset), &new, |percent| {
                    self.change(|state| state.step = Some(UpdateStep::Downloading { percent }));
                })?;
                for (file, new) in besides {
                    swap_over(&file, &new).map_err(|e| format!("{}: {e}", file.display()))?;
                }
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

pub fn mac_release() -> Result<crate::ssh::Release, String> {
    let releases = RELEASES.ok_or("this loom build has no release address (LOOM_RELEASES)")?;
    let version = fetch_latest(releases)?;
    Ok(crate::ssh::Release {
        file: format!("loom-{version}"),
        url: asset_url(releases, &version, MAC_ASSET),
    })
}

fn staged(file: &Path) -> PathBuf {
    file.with_file_name(format!(
        "{}.new",
        file.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn fetch_besides(
    releases: &str,
    version: &str,
    exe: &Path,
    besides: &[Beside],
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    besides
        .iter()
        .map(|beside| {
            let file = exe.with_file_name(beside.file);
            let new = staged(&file);
            download(&asset_url(releases, version, beside.asset), &new, |_| {})?;
            Ok((file, new))
        })
        .collect()
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

fn swap_over(exe: &Path, new: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(new, exe)
}

#[cfg(windows)]
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
