//! Codex app-server: Codex 노드마다 하나를 띄우고 WebSocket JSON-RPC로 잇는다.
//! 노드의 TUI는 `codex --remote <주소>`로 같은 서버에 붙는다. 이 연결은 스레드를 구독하지 않는다.
//! 구독하면 승인 요청도 이 연결로 온다. 구독 없이도 `thread/started`·`thread/status/changed`는 온다.
//! 서버는 노드처럼 데몬이 다시 시작해도 산다. 새 데몬은 DB에 남긴 주소로 다시 붙는다(`reconnect`).
//! 바뀌는 이유는 app-server 전송 방식과 프로세스 모양이다. 요청 내용은 `profile::codex`가 정한다.
//! 근거: docs/design.md 「에이전트 실행」, 「실측 근거」.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::process_tree::{self, Snapshot};
use crate::profile::ServerLink;
use crate::profile::codex::Status;
use crate::profile::codex::protocol::{
    ThreadReadParams, ThreadReadResponse, ThreadStartedNotification, ThreadStatus,
    ThreadStatusChangedNotification,
};

/// 서버가 뜨고 요청 하나에 답하는 상한. 첫 실행은 로그인 확인과 플러그인 읽기로 몇 초 걸린다.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);

/// 노드의 대화 스레드. TUI가 만든다. 서버가 따로 만드는 제목용 임시 스레드는 거른다.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub id: String,
    pub status: Status,
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

/// app-server 하나와 데몬의 연결. 서버 프로세스는 데몬과 따로 산다. 데몬이 다시 시작하면 `reconnect`로 다시 붙는다.
pub struct CodexServer {
    url: String,
    /// 셸을 찾고 서버를 끝내는 뿌리. `codex`는 실행 파일을 띄우는 node 스크립트라 셸은 손자 아래에 뜬다.
    pid: Option<u32>,
    to_server: mpsc::UnboundedSender<Message>,
    pending: Arc<Pending>,
    next_id: AtomicU64,
    thread: watch::Receiver<Option<Thread>>,
    /// `reconnect`가 읽은 스레드 상태를 넣는 길.
    thread_tx: Weak<watch::Sender<Option<Thread>>>,
}

impl CodexServer {
    /// `home`이 `None`이면 시스템 기본 로그인(`~/.codex`)을 쓴다.
    /// `node_env`는 노드 프로세스와 같은 환경이다. 셸 명령은 이 서버 아래에서 돌므로 `loom`이 그 값을 읽는다.
    /// `config`는 서버에 넘길 `-c` 인자다.
    pub async fn start(
        home: Option<&Path>,
        node_env: &[(&'static str, OsString)],
        config: &[String],
    ) -> Result<Self, String> {
        let port = process_tree::free_port()?;
        let url = format!("ws://127.0.0.1:{port}");
        let mut cmd = process_tree::command("codex");
        cmd.envs(node_env.iter().map(|(k, v)| (k, v)));
        cmd.args(config);
        // Codex 셸은 로그인 셸 스냅샷의 `PATH`를 다시 적용해 노드 `PATH`의 `loom` 폴더를 지운다(실측).
        // 셸 환경 설정으로 노드 `PATH`를 다시 넣는다. JSON 문자열은 TOML 문자열로도 읽힌다.
        if let Some((_, path)) = node_env.iter().find(|(key, _)| *key == "PATH") {
            let path = json!(path.to_string_lossy());
            cmd.args(["-c", &format!("shell_environment_policy.set.PATH={path}")]);
        }
        cmd.args(["app-server", "--listen", &url]);
        if let Some(dir) = home {
            cmd.env("CODEX_HOME", dir);
        }
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
        let server = match listening {
            Ok(()) => Self::connect(url, pid).await,
            Err(e) => Err(e),
        };
        server.map_err(|e| {
            if let Some(root) = pid {
                Snapshot::now().kill_trees([root]);
            }
            format!("`codex app-server`: {e}")
        })
    }

    /// 앞 데몬이 띄운 서버에 다시 붙는다. `thread`가 있으면 그 상태를 읽어 둔다.
    /// 상태를 읽지 못하면 쉬는 중으로 둔다. 다음 `thread/status/changed`가 고친다.
    pub async fn reconnect(
        url: String,
        pid: Option<u32>,
        thread: Option<String>,
    ) -> Result<Self, String> {
        let server = Self::connect(url, pid).await?;
        if let Some(id) = thread {
            let status = server
                .read_status(&id)
                .await
                .inspect_err(|e| eprintln!("loom: Codex 스레드 {id}의 상태를 읽지 못했다: {e}"))
                .unwrap_or(Status::Idle);
            if let Some(tx) = server.thread_tx.upgrade() {
                tx.send_replace(Some(Thread { id, status }));
            }
        }
        Ok(server)
    }

    async fn read_status(&self, thread: &str) -> Result<Status, String> {
        let params = ThreadReadParams {
            thread_id: thread.to_owned(),
            include_turns: Some(false),
        };
        let params = serde_json::to_value(params).map_err(|e| e.to_string())?;
        let read = self.call("thread/read", params).await?;
        let read: ThreadReadResponse =
            serde_json::from_value(read).map_err(|e| format!("thread/read: {e}"))?;
        Ok(status(&read.thread.status))
    }

    /// 다시 붙는 데 쓰는 값. DB에 남긴다.
    pub fn link(&self) -> ServerLink {
        ServerLink::Codex {
            url: self.url.clone(),
            pid: self.pid,
            thread: self.thread.borrow().as_ref().map(|t| t.id.clone()),
        }
    }

    /// 연결하고 `initialize`를 보낸다.
    async fn connect(url: String, pid: Option<u32>) -> Result<Self, String> {
        let connected = tokio::time::timeout(
            REPLY_TIMEOUT,
            tokio_tungstenite::connect_async(url.as_str()),
        )
        .await
        .map_err(|_| format!("could not connect to {url}: timed out"))?;
        let (socket, _) = connected.map_err(|e| format!("could not connect to {url}: {e}"))?;
        let (mut sink, mut stream) = socket.split();
        let (to_server, mut outbox) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(msg) = outbox.recv().await {
                if sink.send(msg).await.is_err() {
                    return;
                }
            }
        });
        let pending = Arc::new(Pending::default());
        let (thread_tx, thread) = watch::channel(None);
        // 받는 작업이 유일한 강한 참조다. 연결이 끊겨 작업이 끝나면 송신자가 사라져 구독자도 끝난다.
        let thread_tx = Arc::new(thread_tx);
        let weak_tx = Arc::downgrade(&thread_tx);
        let replies = pending.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(text) = msg
                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                {
                    receive(value, &replies, &thread_tx);
                }
            }
            // 연결이 끊겼다. 기다리는 요청을 모두 실패로 끝낸다.
            for (_, reply) in replies.lock().unwrap().drain() {
                let _ = reply.send(Err("app-server connection closed".into()));
            }
        });
        let server = Self {
            url,
            pid,
            to_server,
            pending,
            next_id: AtomicU64::new(0),
            thread,
            thread_tx: weak_tx,
        };
        let init =
            json!({ "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") } });
        server.call("initialize", init).await?;
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
            return Err("app-server connection closed".into());
        }
        match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply.map_err(|e| format!("{method}: {e}")),
            Ok(Err(_)) => Err(format!("{method}: app-server connection closed")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{method}: no reply"))
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
    }
}

/// 서버가 보낸 메시지 하나. 응답은 기다리는 요청에 넘기고, 스레드 알림은 상태로 바꾼다. 나머지는 버린다.
fn receive(msg: Value, pending: &Pending, thread: &watch::Sender<Option<Thread>>) {
    if msg.get("method").is_none() {
        let Some(id) = msg["id"].as_u64() else { return };
        if let Some(reply) = pending.lock().unwrap().remove(&id) {
            let result = match msg.get("error") {
                Some(error) => Err(error["message"].as_str().unwrap_or("error").to_owned()),
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
