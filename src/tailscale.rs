//! 기반: `tailscale` CLI를 부른다. 폰은 Tailscale Serve로 데몬에 닿는다(docs/design.md 「원격」).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use ts_rs::TS;

/// PATH에 없을 때 보는 자리. WSL에는 Linux판이 없고 Windows판만 있다(2026-10-22 실측: `/mnt/c/Program Files/Tailscale/tailscale.exe`, PATH에 없음).
const INSTALLED: &[&str] = &[
    r"C:\Program Files\Tailscale\tailscale.exe",
    "/mnt/c/Program Files/Tailscale/tailscale.exe",
];

/// `serve --bg`는 보통 1초 안에 끝난다. 이보다 오래 걸리면 끝내고 실패로 알린다. 요청이 끝없이 걸리지 않게 한다.
#[cfg(not(test))]
const SERVE_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const SERVE_TIMEOUT: Duration = Duration::from_secs(2);
/// tailnet에서 Serve를 허용하지 않았으면 CLI가 이 주소의 링크를 찍고 허용될 때까지 기다린다(2026-10-22 실측: Tailscale 1.102.3, `https://login.tailscale.com/f/serve?node=<id>`).
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
        which::which("tailscale")
            .ok()
            .or_else(|| INSTALLED.iter().map(PathBuf::from).find(|p| p.is_file()))
            .map(|program| Self { program })
    }

    #[cfg(all(test, unix))]
    pub fn at(program: &std::path::Path) -> Self {
        Self {
            program: program.to_owned(),
        }
    }

    /// `port`는 데몬이 듣는 포트다.
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

    /// 사람이 [Serve 켜기]를 눌렀을 때만 부른다. Serve는 tailnet 안의 기기만 받는다(Funnel이 아니다).
    /// CLI가 허용 링크를 찍으면 기다리지 않고 그 CLI를 끝낸 뒤 링크를 돌려준다. 다시 눌러도 기다리는 CLI가 쌓이지 않는다.
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

/// `serve status --json`의 `Web.<이름>:443.Handlers.<경로>.Proxy`가 루프백의 `port`를 가리키는지.
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::fake_exe::write_executable;
    use std::path::Path;

    /// 사람의 Tailscale을 부르지 않게 가짜 CLI를 쓴다. 인자를 `calls`에 적고 인자마다 정한 JSON을 낸다.
    fn fake(dir: &Path, status: &str, serve: &str) -> Tailscale {
        let program = dir.join("tailscale");
        write_executable(
            &program,
            &format!(
                "#!/bin/sh\necho \"$@\" >> '{calls}'\ncase \"$1 $2\" in\n  'status --json') cat <<'EOF'\n{status}\nEOF\n;;\n  'serve status') cat <<'EOF'\n{serve}\nEOF\n;;\n  'serve --bg') ;;\n  *) exit 1;;\nesac\n",
                calls = dir.join("calls").display()
            ),
        );
        Tailscale::at(&program)
    }

    const RUNNING: &str = r#"{"BackendState":"Running","Self":{"DNSName":"pc.tail1.ts.net."}}"#;
    const SERVING: &str =
        r#"{"Web":{"pc.tail1.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7420"}}}}}"#;

    #[tokio::test]
    async fn reads_host_and_serve() {
        let dir = tempfile::tempdir().unwrap();
        let ts = fake(dir.path(), RUNNING, SERVING);
        assert_eq!(
            ts.status(7420).await,
            Tailnet::Ready {
                host: "pc.tail1.ts.net".into(),
                serving: true
            }
        );
        assert_eq!(
            ts.status(7421).await,
            Tailnet::Ready {
                host: "pc.tail1.ts.net".into(),
                serving: false
            }
        );
        let ts = fake(dir.path(), RUNNING, "{}");
        assert!(matches!(
            ts.status(7420).await,
            Tailnet::Ready { serving: false, .. }
        ));
    }

    #[tokio::test]
    async fn stopped_when_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let ts = fake(
            dir.path(),
            r#"{"BackendState":"NeedsLogin","Self":{"DNSName":""}}"#,
            "{}",
        );
        assert_eq!(ts.status(7420).await, Tailnet::Stopped);
        let ts = fake(dir.path(), "not json", "{}");
        assert_eq!(ts.status(7420).await, Tailnet::Stopped);
    }

    #[tokio::test]
    async fn serve_runs_background_proxy_to_port() {
        let dir = tempfile::tempdir().unwrap();
        let served = fake(dir.path(), RUNNING, "{}").serve(7421).await;
        assert_eq!(served, Ok(Served::On));
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.trim(), "serve --bg 7421");
    }

    /// `serve --bg` 자리에 `body`를 도는 가짜 CLI.
    fn fake_serve(dir: &Path, body: &str) -> Tailscale {
        let program = dir.join("tailscale");
        write_executable(&program, &format!("#!/bin/sh\n{body}\n"));
        Tailscale::at(&program)
    }

    /// 2026-10-22 사람의 PC에서 Tailscale 1.102.3이 찍은 글 그대로다. 찍은 뒤 끝나지 않는다.
    #[tokio::test]
    async fn serve_returns_consent_link_and_ends_the_waiting_cli() {
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        let ts = fake_serve(
            dir.path(),
            &format!(
                "echo $$ > '{}'\nprintf 'Serve is not enabled on your tailnet.\\nTo enable, visit:\\n\\n         https://login.tailscale.com/f/serve?node=n1\\n'\nexec sleep 600",
                pid.display()
            ),
        );
        assert_eq!(
            ts.serve(7420).await,
            Ok(Served::NeedsConsent {
                url: "https://login.tailscale.com/f/serve?node=n1".into()
            })
        );
        let pid = std::fs::read_to_string(&pid).unwrap();
        let stat = format!("/proc/{}/stat", pid.trim());
        for _ in 0..50 {
            match std::fs::read_to_string(&stat) {
                Err(_) => return,
                Ok(s) if s.split_whitespace().nth(2) == Some("Z") => return,
                Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        panic!("the waiting tailscale serve still runs");
    }

    #[tokio::test]
    async fn serve_reports_failure_text() {
        let dir = tempfile::tempdir().unwrap();
        let ts = fake_serve(dir.path(), "echo 'not logged in' >&2\nexit 1");
        assert_eq!(ts.serve(7420).await, Err("not logged in".into()));
        let ts = fake_serve(dir.path(), "echo 'said on stdout'\nexit 1");
        assert_eq!(ts.serve(7420).await, Err("said on stdout".into()));
    }

    #[tokio::test]
    async fn serve_gives_up_when_the_cli_never_ends() {
        let dir = tempfile::tempdir().unwrap();
        let ts = fake_serve(dir.path(), "exec sleep 600");
        assert!(ts.serve(7420).await.unwrap_err().contains("끝나지 않았다"));
    }
}
