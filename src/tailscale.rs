//! 기반: `tailscale` CLI를 부른다. 폰은 Tailscale Serve로 데몬에 닿는다(docs/design.md 「원격」).

use std::path::PathBuf;

use serde::Serialize;
use tokio::process::Command;
use ts_rs::TS;

/// PATH에 없을 때 보는 자리. WSL에는 Linux판이 없고 Windows판만 있다(2026-10-22 실측: `/mnt/c/Program Files/Tailscale/tailscale.exe`, PATH에 없음).
const INSTALLED: &[&str] = &[
    r"C:\Program Files\Tailscale\tailscale.exe",
    "/mnt/c/Program Files/Tailscale/tailscale.exe",
];

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
    pub async fn serve(&self, port: u16) -> Result<(), String> {
        self.run(&["serve", "--bg", &port.to_string()])
            .await
            .map(drop)
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
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// 사람의 Tailscale을 부르지 않게 가짜 CLI를 쓴다. 인자를 `calls`에 적고 인자마다 정한 JSON을 낸다.
    fn fake(dir: &Path, status: &str, serve: &str) -> Tailscale {
        let program = dir.join("tailscale");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{calls}'\ncase \"$1 $2\" in\n  'status --json') cat <<'EOF'\n{status}\nEOF\n;;\n  'serve status') cat <<'EOF'\n{serve}\nEOF\n;;\n  'serve --bg') ;;\n  *) exit 1;;\nesac\n",
                calls = dir.join("calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        fake(dir.path(), RUNNING, "{}").serve(7421).await.unwrap();
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.trim(), "serve --bg 7421");
    }
}
