use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use ts_rs::TS;

const INSTALLED: &[&str] = &[
    r"C:\Program Files\Tailscale\tailscale.exe",
    "/mnt/c/Program Files/Tailscale/tailscale.exe",
];

const SERVE_TIMEOUT: Duration = Duration::from_secs(20);
const CONSENT_PREFIX: &str = "https://login.tailscale.com/";

/// [Serve 켜기]의 결과.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Served {
    On,
    /// tailnet 관리자가 Serve를 허용해야 한다. `url`은 CLI가 찍은 허용 페이지다. 허용한 뒤 다시 누른다.
    NeedsConsent {
        url: String,
    },
}

/// 이 컴퓨터의 Tailscale 상태.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Tailnet {
    /// CLI를 찾지 못했다.
    Missing,
    /// CLI는 있지만 tailnet에 붙지 않았다(로그아웃, 꺼짐).
    Stopped,
    /// `host`는 이 기기의 HTTPS 이름(`<기기>.<tailnet>.ts.net`)이다. `serving`은 Serve가 데몬 포트로 넘기는지다.
    Ready { host: String, serving: bool },
}

pub struct Tailscale {
    program: PathBuf,
}

impl Tailscale {
    pub fn find() -> Option<Self> {
        crate::search_path::find("tailscale")
            .or_else(|| INSTALLED.iter().map(PathBuf::from).find(|p| p.is_file()))
            .map(|program| Self { program })
    }

    pub async fn status(&self, port: u16) -> Tailnet {
        let Ok(status) = self.json(&["status", "--json"]).await else {
            return Tailnet::Stopped;
        };
        let host = status["Self"]["DNSName"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('.');
        if status["BackendState"] != "Running" || host.is_empty() {
            return Tailnet::Stopped;
        }
        let serving = self
            .json(&["serve", "status", "--json"])
            .await
            .is_ok_and(|serve| proxies_to(&serve, port));
        Tailnet::Ready {
            host: host.to_owned(),
            serving,
        }
    }

    pub async fn serve(&self, port: u16) -> Result<Served, String> {
        let mut child = Command::new(&self.program)
            .args(["serve", "--bg", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("tailscale을 실행하지 못했다: {e}"))?;
        let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
        let watch = async {
            let mut said = String::new();
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                if let Some(url) = line
                    .split_whitespace()
                    .find(|w| w.starts_with(CONSENT_PREFIX))
                {
                    return Ok(Served::NeedsConsent {
                        url: url.to_owned(),
                    });
                }
                said.push_str(&line);
                said.push('\n');
            }
            let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
            if out.status.success() {
                return Ok(Served::On);
            }
            let stderr = String::from_utf8_lossy(&out.stderr);
            Err(if stderr.trim().is_empty() {
                said
            } else {
                stderr.into_owned()
            }
            .trim()
            .to_owned())
        };
        tokio::time::timeout(SERVE_TIMEOUT, watch)
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "tailscale serve가 {}초 안에 끝나지 않았다",
                    SERVE_TIMEOUT.as_secs()
                ))
            })
    }

    async fn json(&self, args: &[&str]) -> Result<serde_json::Value, String> {
        serde_json::from_str(&self.run(args).await?).map_err(|e| e.to_string())
    }

    async fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(&self.program)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("tailscale을 실행하지 못했다: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn proxies_to(serve: &serde_json::Value, port: u16) -> bool {
    let targets = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        port.to_string(),
    ];
    serve["Web"]
        .as_object()
        .into_iter()
        .flat_map(|web| web.values())
        .filter_map(|site| site["Handlers"].as_object())
        .flat_map(|handlers| handlers.values())
        .filter_map(|handler| handler["Proxy"].as_str())
        .any(|proxy| targets.iter().any(|t| proxy.trim_end_matches('/') == t))
}
