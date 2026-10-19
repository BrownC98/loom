//! Codex app-server: Codex 노드마다 하나를 띄우고 WebSocket JSON-RPC로 잇는다.
//! 노드의 TUI는 `codex --remote <주소>`로 같은 서버에 붙는다. 이 연결은 스레드를 구독하지 않는다.
//! 구독하면 승인 요청도 이 연결로 온다. 구독 없이도 `thread/started`·`thread/status/changed`는 온다.
//! 바뀌는 이유는 app-server 전송 방식과 프로세스 모양이다. 요청 내용은 `profile::codex`가 정한다.
//! 근거: docs/design.md 「에이전트 실행」, 「실측 근거」.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::process_tree::Snapshot;
use crate::profile::codex::Status;
use crate::profile::codex::protocol::{
    ThreadStartedNotification, ThreadStatus, ThreadStatusChangedNotification,
};

/// 서버가 뜨고 요청 하나에 답하는 상한. 첫 실행은 로그인 확인과 플러그인 읽기로 몇 초 걸린다.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// 서버가 듣는 주소를 stderr에 쓰는 줄의 앞머리. 포트는 OS가 고른다(`ws://127.0.0.1:0`).
const LISTENING: &str = "listening on: ";

/// 노드의 대화 스레드. TUI가 만든다. 서버가 따로 만드는 제목용 임시 스레드는 거른다.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub id: String,
    pub status: Status,
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

pub struct CodexServer {
    url: String,
    child: Mutex<Child>,
    /// 셸을 찾는 뿌리. `codex`는 실행 파일을 띄우는 node 스크립트라 셸은 손자 아래에 뜬다.
    pid: Option<u32>,
    to_server: mpsc::UnboundedSender<Message>,
    pending: std::sync::Arc<Pending>,
    next_id: AtomicU64,
    thread: watch::Receiver<Option<Thread>>,
}

impl CodexServer {
    /// `home`이 `None`이면 시스템 기본 로그인(`~/.codex`)을 쓴다.
    /// `node_env`는 노드 프로세스와 같은 환경이다. 셸 명령은 이 서버 아래에서 돌므로 `loom`이 그 값을 읽는다.
    pub async fn start(
        home: Option<&Path>,
        node_env: &[(&'static str, OsString)],
    ) -> Result<Self, String> {
        let mut cmd = Command::new("codex");
        cmd.envs(node_env.iter().map(|(k, v)| (k, v)));
        // Codex 셸은 로그인 셸 스냅샷의 `PATH`를 다시 적용해 노드 `PATH`의 `loom` 폴더를 지운다(실측).
        // 셸 환경 설정으로 노드 `PATH`를 다시 넣는다. JSON 문자열은 TOML 문자열로도 읽힌다.
        if let Some((_, path)) = node_env.iter().find(|(key, _)| *key == "PATH") {
            let path = json!(path.to_string_lossy());
            cmd.args(["-c", &format!("shell_environment_policy.set.PATH={path}")]);
        }
        cmd.args(["app-server", "--listen", "ws://127.0.0.1:0"]);
        if let Some(dir) = home {
            cmd.env("CODEX_HOME", dir);
        }
        #[cfg(target_os = "linux")]
        end_with_daemon(&mut cmd);
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("`codex app-server`를 띄우지 못했다: {e}"))?;
        let pid = child.id();
        let stderr = child.stderr.take().expect("stderr를 piped로 열었다");
        let server = tokio::time::timeout(REPLY_TIMEOUT, connect(stderr)).await;
        let (url, socket) = match server {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => return Err(stop_after(child, pid, e)),
            Err(_) => return Err(stop_after(child, pid, "주소를 알리지 않았다".into())),
        };
        let (mut sink, mut stream) = socket.split();
        let (to_server, mut outbox) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(msg) = outbox.recv().await {
                if sink.send(msg).await.is_err() {
                    return;
                }
            }
        });
        let pending = std::sync::Arc::new(Pending::default());
        let (thread_tx, thread) = watch::channel(None);
        let replies = pending.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(text) = msg
                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                {
                    receive(value, &replies, &thread_tx);
                }
            }
            // 연결이 끊겼다. 기다리는 요청을 모두 실패로 끝낸다. `thread_tx`가 사라져 구독자도 끝난다.
            for (_, reply) in replies.lock().unwrap().drain() {
                let _ = reply.send(Err("app-server 연결이 끊겼다".into()));
            }
        });
        let server = Self {
            url,
            child: Mutex::new(child),
            pid,
            to_server,
            pending,
            next_id: AtomicU64::new(0),
            thread,
        };
        let init =
            json!({ "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") } });
        if let Err(e) = server.call("initialize", init).await {
            server.stop();
            return Err(e);
        }
        server.notify("initialized");
        Ok(server)
    }

    /// TUI가 `--remote`로 붙는 주소.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// 노드 스레드와 그 상태. 서버가 끝나면 `changed()`가 실패한다.
    pub fn thread(&self) -> watch::Receiver<Option<Thread>> {
        self.thread.clone()
    }

    /// JSON-RPC 요청 하나를 보내고 `result`를 돌려준다. `error` 응답은 그 메시지를 돌려준다.
    pub async fn call(&self, method: &'static str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let request = json!({ "method": method, "id": id, "params": params });
        if self
            .to_server
            .send(Message::Text(request.to_string().into()))
            .is_err()
        {
            self.pending.lock().unwrap().remove(&id);
            return Err("app-server 연결이 끊겼다".into());
        }
        match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply.map_err(|e| format!("{method}: {e}")),
            Ok(Err(_)) => Err(format!("{method}: app-server 연결이 끊겼다")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{method}: 응답이 없다"))
            }
        }
    }

    fn notify(&self, method: &str) {
        let _ = self.to_server.send(Message::Text(
            json!({ "method": method }).to_string().into(),
        ));
    }

    /// 돌고 있는 셸 명령과 그 자손을 끝내고 끝낸 프로세스 수를 돌려준다.
    /// `turn/interrupt`는 턴만 멈추고 셸은 끝까지 돈다. 셸은 서버 아래의 세션 리더다.
    /// MCP 서버와 `codex-code-mode-host`는 서버의 세션 안에 있어 끝내지 않는다.
    pub fn stop_shells(&self) -> usize {
        let Some(root) = self.pid else { return 0 };
        let snapshot = Snapshot::now();
        snapshot.kill_trees(snapshot.session_leaders_under(root))
    }

    /// 서버와 그 아래 프로세스를 모두 끝낸다.
    pub fn stop(&self) {
        if let Some(root) = self.pid {
            Snapshot::now().kill_trees([root]);
        }
        // 이미 끝났으면 실패한다. 할 일이 없다.
        let _ = self.child.lock().unwrap().start_kill();
    }
}

/// 데몬이 어떻게 끝나든(신호, 충돌) 서버도 TERM을 받는다. `codex` wrapper는 TERM을 실행 파일에 넘긴다.
/// 표준 입력이 닫혀도 서버는 끝나지 않는다(실측). 노드 PTY와 달리 끊김을 알 길이 이것뿐이다.
/// 신호는 띄운 스레드가 끝날 때 간다. 노드는 tokio 작업 스레드에서 띄우고, 그 스레드는 데몬이 끝날 때까지 산다.
#[cfg(target_os = "linux")]
fn end_with_daemon(cmd: &mut Command) {
    // SAFETY: fork 뒤 exec 전에 async-signal-safe인 prctl 하나만 부른다.
    unsafe {
        cmd.pre_exec(
            || match libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) {
                -1 => Err(std::io::Error::last_os_error()),
                _ => Ok(()),
            },
        );
    }
}

/// stderr에서 듣는 주소를 찾아 연결한다. 그 뒤의 stderr도 계속 읽어 버린다. 읽지 않으면 파이프가 차서 서버가 멈춘다.
async fn connect(
    stderr: tokio::process::ChildStderr,
) -> Result<
    (
        String,
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ),
    String,
> {
    let mut lines = BufReader::new(stderr).lines();
    let url = loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(url) = line.split_once(LISTENING).map(|(_, u)| u.trim().to_owned()) {
                    break url;
                }
            }
            _ => return Err("주소를 알리기 전에 끝났다".into()),
        }
    };
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .map_err(|e| format!("{url}에 연결하지 못했다: {e}"))?;
    Ok((url, socket))
}

fn stop_after(mut child: Child, pid: Option<u32>, why: String) -> String {
    if let Some(root) = pid {
        Snapshot::now().kill_trees([root]);
    }
    let _ = child.start_kill();
    format!("`codex app-server`: {why}")
}

/// 서버가 보낸 메시지 하나. 응답은 기다리는 요청에 넘기고, 스레드 알림은 상태로 바꾼다. 나머지는 버린다.
fn receive(msg: Value, pending: &Pending, thread: &watch::Sender<Option<Thread>>) {
    if msg.get("method").is_none() {
        let Some(id) = msg["id"].as_u64() else { return };
        if let Some(reply) = pending.lock().unwrap().remove(&id) {
            let result = match msg.get("error") {
                Some(error) => Err(error["message"].as_str().unwrap_or("오류").to_owned()),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = reply.send(result);
        }
        return;
    }
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match msg["method"].as_str() {
        Some("thread/started") => {
            let Ok(started) = serde_json::from_value::<ThreadStartedNotification>(params) else {
                return;
            };
            if !started.thread.ephemeral {
                thread.send_replace(Some(Thread {
                    id: started.thread.id,
                    status: status(&started.thread.status),
                }));
            }
        }
        Some("thread/status/changed") => {
            let Ok(changed) = serde_json::from_value::<ThreadStatusChangedNotification>(params)
            else {
                return;
            };
            thread.send_if_modified(|now| match now {
                Some(t) if t.id == changed.thread_id => {
                    t.status = status(&changed.status);
                    true
                }
                _ => false,
            });
        }
        _ => {}
    }
}

fn status(status: &ThreadStatus) -> Status {
    match status {
        ThreadStatus::Active(_) => Status::Busy,
        ThreadStatus::Idle | ThreadStatus::NotLoaded | ThreadStatus::SystemError => Status::Idle,
    }
}
