//! OpenCode 2 서버 프로세스와 HTTP 연결. 노드마다 `opencode2 serve` 하나를 띄운다.
//! 서버는 노드처럼 데몬이 다시 시작해도 산다. 새 데몬은 DB에 남긴 주소·비밀번호로 다시 붙는다(`connect`).
//! 바뀌는 이유는 서버의 시작 방식과 API 경로다. 무엇을 언제 보낼지는 `mod.rs`가 정한다.
//! API 근거: `opencode2 serve`가 내는 `/openapi.json`(2.0.32). docs/design.md 「실측 근거」.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::watch;

use crate::process_tree::{self, Snapshot};

/// 서버가 뜨고 요청 하나에 답하는 상한. 첫 실행은 플러그인·모델 목록을 읽어 몇 초 걸린다.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// 서버가 듣기 시작한 뒤 준비될 때까지 다시 묻는 간격. 실측(2.0.32): 듣기와 준비 사이가 약 50ms다.
/// 서버가 주는 `retry-after: 1`은 1초라 노드를 띄울 때마다 1초를 버린다. 그래서 짧게 묻는다.
const READY_POLL: Duration = Duration::from_millis(50);
/// Basic 인증의 사용자 이름. 서버가 정한 값이다(실측: 다른 이름과 Bearer는 401).
const USER: &str = "opencode";
/// SSE 이벤트 본문 줄의 앞머리. 서버는 이벤트 하나를 `data:` 한 줄로 쓴다(실측).
const SSE_DATA: &str = "data: ";

/// 서버 하나와 데몬의 연결. 서버 프로세스는 데몬과 따로 산다. 데몬이 다시 시작하면 `connect`로 다시 붙는다.
pub struct Server {
    url: String,
    password: String,
    /// `Authorization` 머리글 값.
    auth: String,
    /// 셸을 찾고 서버를 끝내는 뿌리.
    pid: Option<u32>,
    /// 이벤트 연결이 끊기면(서버가 끝나면) `true`가 된다.
    stopped: Arc<watch::Sender<bool>>,
}

impl Server {
    /// `cwd`에서 서버를 띄우고 들을 때까지 기다린다. 서버는 HTTP Basic `password`만 받는다.
    pub async fn start(
        program: &str,
        cwd: &Path,
        node_env: &[(&'static str, OsString)],
        password: String,
    ) -> Result<Self, String> {
        let port = process_tree::free_port()?;
        let mut cmd = process_tree::command(program);
        cmd.args([
            "serve",
            "--hostname",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .current_dir(cwd)
        .envs(node_env.iter().map(|(k, v)| (k, v)))
        .env(super::PASSWORD_ENV, &password);
        // 노드가 데몬보다 오래 살므로 서버도 따로 산다. 출력은 읽지 않는다. 데몬이 끝나면 읽을 쪽이 없다.
        process_tree::detach(&mut cmd);
        let mut child = Command::from(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let pid = child.id();
        let listening = tokio::time::timeout(
            REPLY_TIMEOUT,
            process_tree::until_listening(&mut child, port),
        )
        .await
        .unwrap_or_else(|_| Err("did not listen".into()));
        // 이 데몬이 띄운 서버는 끝날 때 이 데몬이 거둔다.
        tokio::spawn(async move { child.wait().await });
        let server = Self::connect(format!("http://127.0.0.1:{port}"), password, pid);
        // 듣기 시작과 첫 요청을 받는 때가 다르다. 듣기만 보고 요청을 보내면 503 `service_starting`을 받는다.
        let ready = match listening {
            Ok(()) => server.ready().await,
            Err(e) => Err(e),
        };
        if let Err(e) = ready {
            server.stop();
            return Err(e);
        }
        Ok(server)
    }

    /// 서버가 듣기 시작했지만 기동 중이면 503을 준다. 실측(2.0.32): `/api/info`는 그때 정보 본문과 `503`을 함께 주고,
    /// 다른 경로는 `{"code":"service_starting"}`과 함께 준다. 그 창은 약 50ms다.
    /// 그래서 첫 요청 전에 `/api/info`로 준비를 기다린다. 이 경로는 기동 중에도 답한다(실측).
    /// 503이 아닌 거부는 바로 돌려준다. 서버가 실패한 뒤 15초를 버리지 않는다.
    async fn ready(&self) -> Result<(), String> {
        let info = format!("{}/api/info", self.url);
        let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
        loop {
            let url = info.clone();
            let auth = self.auth.clone();
            let asked = tokio::task::spawn_blocking(move || request("GET", &url, &auth, None))
                .await
                .map_err(|e| e.to_string())??;
            if (200..300).contains(&asked.status) {
                return Ok(());
            }
            if asked.status != 503 {
                return Err(asked.refusal());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("did not get ready: {}", asked.refusal()));
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }

    /// 듣고 있는 서버에 붙는다. 요청을 보내기 전에는 서버를 부르지 않는다.
    pub fn connect(url: String, password: String, pid: Option<u32>) -> Self {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{password}"));
        Self {
            url,
            password,
            auth: format!("Basic {basic}"),
            pid,
            stopped: Arc::new(watch::channel(false).0),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// 이벤트 연결이 끊기면(서버가 끝나면) `true`가 된다.
    pub fn stopped(&self) -> watch::Receiver<bool> {
        self.stopped.subscribe()
    }

    /// 서버 이벤트(`GET /api/event`, SSE)를 받을 때마다 `on_event`에 `data` JSON을 넘기는 스레드를 띄운다.
    /// 서버가 끝나 연결이 닫히면 스레드도 끝나고 `stopped`가 `true`가 된다.
    /// 다시 시작한 데몬은 서버의 자식이 아니라 기다릴 수 없다. 그래서 끝남을 이 연결로 안다.
    pub fn follow_events(&self, mut on_event: impl FnMut(Value) + Send + 'static) {
        let url = format!("{}/api/event", self.url);
        let auth = self.auth.clone();
        let stopped = self.stopped.clone();
        std::thread::spawn(move || {
            let _ended = Ended(stopped);
            // 이벤트는 끝없이 오므로 응답 상한을 두지 않는다.
            let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
            let Ok(mut res) = agent.get(&url).header("authorization", &auth).call() else {
                return;
            };
            let lines =
                std::io::BufRead::lines(std::io::BufReader::new(res.body_mut().as_reader()));
            for line in lines.map_while(Result::ok) {
                if let Some(Ok(event)) = line.strip_prefix(SSE_DATA).map(serde_json::from_str) {
                    on_event(event);
                }
            }
        });
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

    /// 서버와 그 아래 프로세스를 모두 끝낸다. 이벤트 연결이 끊겨 `stopped`가 `true`가 된다.
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
            .flatten()
            .and_then(Reply::json);
        reply.map_err(|e| format!("{what}: {e}"))
    }
}

fn request(method: &str, url: &str, auth: &str, body: Option<Value>) -> Result<Reply, String> {
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
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    Ok(Reply { status, text })
}

/// 서버의 답 하나. 상태 코드를 남겨 기동 중(503)과 진짜 거부를 가른다.
struct Reply {
    status: u16,
    text: String,
}

impl Reply {
    /// 응답 본문의 JSON. 본문이 없으면(204) `null`이다.
    fn json(self) -> Result<Value, String> {
        if !(200..300).contains(&self.status) {
            return Err(self.refusal());
        }
        if self.text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&self.text).map_err(|e| format!("unknown reply: {e}"))
    }

    /// 화면·로그에 보일 거부 문장. `loom: Could not start ...: <이 문장>`이 된다.
    fn refusal(&self) -> String {
        format!("{} {}", self.status, self.text)
    }
}

/// 버려질 때 `stopped`를 `true`로 둔다. 이벤트 스레드가 어느 길로 끝나든 알린다.
struct Ended(Arc<watch::Sender<bool>>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 정해진 답을 차례로 주는 서버. 답이 떨어지면 마지막 답을 다시 준다. 받은 요청 수를 센다.
    /// 서버는 `Connection: close`로 답하므로 연결마다 요청 하나다.
    fn fake_server(replies: &[(u16, &'static str)]) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(AtomicUsize::new(0));
        let seen = asked.clone();
        let replies: Vec<(u16, String)> =
            replies.iter().map(|(s, b)| (*s, b.to_string())).collect();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                read_request(&mut stream);
                let n = seen.fetch_add(1, Ordering::Relaxed);
                let (status, body) = &replies[n.min(replies.len() - 1)];
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (url, asked)
    }

    /// 요청을 끝까지(머리와 `content-length`만큼의 본문) 읽는다. 읽지 않은 바이트를 두고 닫으면
    /// Windows가 연결을 끊어(RST) 클라이언트가 답 대신 os error 10053을 받는다(2026-10-22 Windows 실측).
    fn read_request(stream: &mut std::net::TcpStream) {
        let mut got = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
                let body = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if got.len() >= end + 4 + body {
                    return;
                }
            }
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => got.extend_from_slice(&buf[..n]),
            }
        }
    }

    /// 듣기 시작과 첫 요청을 받는 때가 다르다. 그 창에 `/api/info`는 정보 본문과 503을 함께 준다(실측).
    /// 그래서 띄우기는 준비될 때까지 다시 묻고, 그 뒤 첫 요청을 보낸다.
    #[tokio::test]
    async fn the_starting_window_is_waited_out_before_the_first_request() {
        let (url, asked) = fake_server(&[
            (503, r#"{"version":"2.0.32","pid":1}"#),
            (
                200,
                r#"{"version":"2.0.32","pid":1,"capabilities":{"persistentPty":true}}"#,
            ),
        ]);
        let server = Server::connect(url, "pw".into(), None);
        server.ready().await.unwrap();
        assert_eq!(asked.load(Ordering::Relaxed), 2);
    }

    /// 기동 중이 아닌 거부는 다시 묻지 않고 바로 알린다. 끝난 서버를 15초 동안 기다리지 않는다.
    #[tokio::test]
    async fn a_service_that_failed_is_reported_without_another_try() {
        let (url, asked) = fake_server(&[(
            500,
            r#"{"code":"service_failed","message":"The background service could not start."}"#,
        )]);
        let server = Server::connect(url, "pw".into(), None);
        let err = server.ready().await.unwrap_err();
        assert!(err.contains("service_failed"), "{err}");
        assert_eq!(asked.load(Ordering::Relaxed), 1);
    }

    /// 다른 오류의 답 문장은 예전과 같다. 화면에 보이는 `Could not start ...: <상태> <본문>`이 바뀌지 않는다.
    #[tokio::test]
    async fn other_errors_keep_the_old_message_shape() {
        let (url, _) = fake_server(&[(401, r#"{"code":"unauthorized"}"#)]);
        let server = Server::connect(url, "pw".into(), None);
        let err = server.create_session(Path::new(".")).await.unwrap_err();
        assert_eq!(err, r#"POST /api/session: 401 {"code":"unauthorized"}"#);
    }
}
