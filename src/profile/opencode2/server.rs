//! OpenCode 2 서버 프로세스와 HTTP 연결. 노드마다 `opencode2 serve` 하나를 띄운다.
//! 바뀌는 이유는 서버의 시작 방식과 API 경로다. 무엇을 언제 보낼지는 `mod.rs`가 정한다.
//! API 근거: `opencode2 serve`가 내는 `/openapi.json`(2.0.32). docs/design.md 「실측 근거」.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

use crate::process_tree::Snapshot;

/// 서버가 뜨고 요청 하나에 답하는 상한. 첫 실행은 플러그인·모델 목록을 읽어 몇 초 걸린다.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// 서버가 듣는 주소를 stdout에 쓰는 줄의 앞머리. 포트는 OS가 고른다(`--port 0`).
const LISTENING: &str = "server listening on ";
/// Basic 인증의 사용자 이름. 서버가 정한 값이다(실측: 다른 이름과 Bearer는 401).
const USER: &str = "opencode";

pub struct Server {
    url: String,
    password: String,
    /// `Authorization` 머리글 값.
    auth: String,
    /// 셸을 찾는 뿌리.
    pid: Option<u32>,
    /// 서버 프로세스가 끝나면 `true`가 된다.
    stopped: watch::Receiver<bool>,
}

impl Server {
    /// `cwd`에서 서버를 띄우고 듣는 주소를 기다린다. 서버는 HTTP Basic `password`만 받는다.
    pub async fn start(
        program: &str,
        cwd: &Path,
        node_env: &[(&'static str, OsString)],
        password: String,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(program);
        cmd.args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
            .current_dir(cwd)
            .envs(node_env.iter().map(|(k, v)| (k, v)))
            .env(super::PASSWORD_ENV, &password);
        #[cfg(target_os = "linux")]
        crate::process_tree::end_with_daemon(&mut cmd);
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| e.to_string())?;
        let pid = child.id();
        let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
        let url = match tokio::time::timeout(REPLY_TIMEOUT, listening(stdout)).await {
            Ok(Ok(url)) => url,
            failed => {
                if let Some(root) = pid {
                    Snapshot::now().kill_trees([root]);
                }
                let why = match failed {
                    Ok(Err(e)) => e,
                    _ => "did not announce its address".into(),
                };
                return Err(why);
            }
        };
        let (done, stopped) = watch::channel(false);
        tokio::spawn(async move {
            let _ = child.wait().await;
            done.send_replace(true);
        });
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{password}"));
        Ok(Self {
            url,
            password,
            auth: format!("Basic {basic}"),
            pid,
            stopped,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// 서버 프로세스가 끝나면 `true`가 된다.
    pub fn stopped(&self) -> watch::Receiver<bool> {
        self.stopped.clone()
    }

    /// `cwd`를 작업 폴더로 하는 새 세션의 id.
    pub async fn create_session(&self, cwd: &Path) -> Result<String, String> {
        // 노드에는 승인할 사람이 없다. 사용자 설정이 `ask`로 두어도 이 세션은 모두 허용한다. 근거: docs/design.md 「에이전트 실행」.
        let body = json!({
            "location": { "directory": cwd },
            "permissions": [{ "action": "*", "resource": "*", "effect": "allow" }],
        });
        let reply = self.call("POST", "/api/session", Some(body)).await?;
        reply["data"]["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("session create: unknown reply: {reply}"))
    }

    /// 세션의 지시 항목 `key`를 `value`로 둔다. 모델은 다음 단계부터 본다.
    pub async fn put_instructions(
        &self,
        session: &str,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let path = format!("/api/experimental/session/{session}/instructions/entries/{key}");
        self.call("PUT", &path, Some(json!({ "value": value })))
            .await
            .map(drop)
    }

    /// 세션 입력함에 메시지 하나를 넣는다. `delivery`는 `steer` 또는 `queue`다.
    pub async fn prompt(
        &self,
        session: &str,
        id: &str,
        text: &str,
        delivery: &str,
    ) -> Result<(), String> {
        let body = json!({ "id": id, "text": text, "delivery": delivery });
        let path = format!("/api/session/{session}/prompt");
        self.call("POST", &path, Some(body)).await.map(drop)
    }

    /// 세션의 실행을 멈춘다. 쉬는 중이면 아무것도 하지 않는다.
    pub async fn interrupt(&self, session: &str) -> Result<(), String> {
        let path = format!("/api/session/{session}/interrupt");
        self.call("POST", &path, None).await.map(drop)
    }

    /// 세션의 돌고 있는 셸 명령과 그 자손을 끝낸다. 서버 아래 프로세스만 끝낸다.
    /// 목록을 읽지 못하면 아무것도 끝내지 않는다. 그래도 `interrupt`가 셸의 프로세스 그룹은 끝낸다(실측).
    pub async fn stop_shells(&self, session: &str) {
        let Some(root) = self.pid else { return };
        let shells = match self.call("GET", "/api/shell", None).await {
            Ok(reply) => reply,
            Err(e) => return eprintln!("loom: OpenCode 2 셸 목록을 읽지 못했다: {e}"),
        };
        let pids: Vec<u32> = shells["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["status"] == "running" && s["metadata"]["sessionID"] == session)
            .filter_map(|s| s["pid"].as_u64().map(|p| p as u32))
            .collect();
        let snapshot = Snapshot::now();
        snapshot.kill_trees(pids.into_iter().filter(|&p| snapshot.descends(p, root)));
    }

    /// 서버와 그 아래 프로세스를 모두 끝낸다. `stopped`가 `true`가 된다.
    pub fn stop(&self) {
        if let Some(root) = self.pid {
            Snapshot::now().kill_trees([root]);
        }
    }

    /// HTTP 요청 하나를 보내고 JSON 응답을 돌려준다. 본문이 없는 응답(204)은 `null`이다.
    async fn call(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let url = format!("{}{path}", self.url);
        let auth = self.auth.clone();
        let what = format!("{method} {path}");
        let reply = tokio::task::spawn_blocking(move || request(method, &url, &auth, body))
            .await
            .map_err(|e| e.to_string())
            .flatten();
        reply.map_err(|e| format!("{what}: {e}"))
    }
}

fn request(method: &str, url: &str, auth: &str, body: Option<Value>) -> Result<Value, String> {
    // 거부 문장을 본문에서 읽으려고 4xx·5xx를 오류로 바꾸지 않는다.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(REPLY_TIMEOUT))
        .build()
        .into();
    let request = ureq::http::Request::builder()
        .method(method)
        .uri(url)
        .header("authorization", auth);
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(body.to_string()),
        None => request.body(String::new()),
    }
    .map_err(|e| e.to_string())?;
    let mut res = agent.run(request).map_err(|e| e.to_string())?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("{} {text}", res.status()));
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("unknown reply: {e}"))
}

/// stdout에서 듣는 주소를 찾는다. 그 뒤의 stdout도 계속 읽어 버린다. 읽지 않으면 파이프가 차서 서버가 멈춘다.
async fn listening(stdout: tokio::process::ChildStdout) -> Result<String, String> {
    let mut lines = BufReader::new(stdout).lines();
    let url = loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some((_, url)) = line.split_once(LISTENING) {
                    break url.trim().trim_end_matches('/').to_owned();
                }
            }
            _ => return Err("exited before announcing its address".into()),
        }
    };
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Ok(url)
}
