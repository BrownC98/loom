use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use super::Status;
use super::protocol::{
    ThreadActiveFlag, ThreadReadParams, ThreadReadResponse, ThreadStartedNotification,
    ThreadStatus, ThreadStatusChangedNotification,
};
use crate::process_tree::{self, Snapshot};
use crate::profile::process::Served;
use crate::protocol::{ServerLink, Session, WaitKind};
use crate::wsl::Inside;

const REPLY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub id: String,
    pub status: Status,
    pub waiting: Option<WaitKind>,
}

impl Thread {
    fn new(id: String, status: &ThreadStatus) -> Self {
        let waiting = match status {
            ThreadStatus::Active(flags) => flags.first().map(|f| match f {
                ThreadActiveFlag::WaitingOnApproval => WaitKind::Approval,
                ThreadActiveFlag::WaitingOnUserInput => WaitKind::Question,
            }),
            _ => None,
        };
        Self {
            id,
            status: match status {
                ThreadStatus::Active(_) => Status::Busy,
                ThreadStatus::Idle | ThreadStatus::NotLoaded | ThreadStatus::SystemError => {
                    Status::Idle
                }
            },
            waiting,
        }
    }
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

pub struct CodexServer {
    url: String,
    process: Served,
    to_server: mpsc::UnboundedSender<Message>,
    pending: Arc<Pending>,
    next_id: AtomicU64,
    thread: watch::Receiver<Option<Thread>>,
    thread_tx: Weak<watch::Sender<Option<Thread>>>,
    limited: watch::Receiver<u64>,
}

impl CodexServer {
    pub async fn start(
        cwd: &Path,
        inside: Option<&Inside>,
        node_env: &[(&'static str, OsString)],
        config: &[String],
    ) -> Result<Self, String> {
        let port = process_tree::free_port()?;
        let url = format!("ws://127.0.0.1:{port}");
        let mut args = config.to_vec();
        let path = match inside {
            Some(inside) => Some(inside.node_path()),
            None => node_env
                .iter()
                .find(|(key, _)| *key == "PATH")
                .map(|(_, path)| path.to_string_lossy().into_owned()),
        };
        if let Some(path) = path {
            args.extend([
                "-c".into(),
                format!("shell_environment_policy.set.PATH={}", json!(path)),
            ]);
        }
        args.extend(["app-server".into(), "--listen".into(), url.clone()]);
        let argv = [&["codex".to_owned()], &args[..]].concat();
        let failed = |e| format!("`codex app-server`: {e}");
        let process = Served::start(&argv, cwd, inside, node_env, port, REPLY_TIMEOUT)
            .await
            .map_err(failed)?;
        Self::connect(url, process.clone()).await.map_err(|e| {
            process.stop();
            failed(e)
        })
    }

    pub async fn reconnect(
        url: String,
        process: Served,
        thread: Option<&Session>,
    ) -> Result<Self, String> {
        let server = Self::connect(url, process).await?;
        if let Some(id) = thread {
            server.follow_thread(id.0.clone()).await;
        }
        Ok(server)
    }

    pub async fn follow_thread(&self, id: String) {
        let status = self
            .read(&id)
            .await
            .map(|read| read.thread.status)
            .inspect_err(|e| eprintln!("loom: Codex 스레드 {id}의 상태를 읽지 못했다: {e}"))
            .unwrap_or(ThreadStatus::Idle);
        if let Some(tx) = self.thread_tx.upgrade() {
            tx.send_replace(Some(Thread::new(id, &status)));
        }
    }

    pub async fn rollout(&self, thread: &str) -> Result<Option<String>, String> {
        Ok(self.read(thread).await?.thread.path)
    }

    pub fn limited(&self) -> watch::Receiver<u64> {
        self.limited.clone()
    }

    async fn read(&self, thread: &str) -> Result<ThreadReadResponse, String> {
        let params = ThreadReadParams {
            thread_id: thread.to_owned(),
            include_turns: Some(false),
        };
        let params = serde_json::to_value(params).map_err(|e| e.to_string())?;
        let read = self.call("thread/read", params).await?;
        serde_json::from_value(read).map_err(|e| format!("thread/read: {e}"))
    }

    pub fn link(&self) -> ServerLink {
        ServerLink::Codex {
            url: self.url.clone(),
            pid: self.process.pid,
        }
    }

    async fn connect(url: String, process: Served) -> Result<Self, String> {
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
        let thread_tx = Arc::new(thread_tx);
        let weak_tx = Arc::downgrade(&thread_tx);
        let (limited_tx, limited) = watch::channel(0);
        let replies = pending.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(text) = msg
                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                {
                    if is_usage_limit(&value) {
                        limited_tx.send_modify(|n| *n += 1);
                    }
                    receive(value, &replies, &thread_tx);
                }
            }
            for (_, reply) in replies.lock().unwrap().drain() {
                let _ = reply.send(Err("app-server connection closed".into()));
            }
        });
        let server = Self {
            url,
            process,
            to_server,
            pending,
            next_id: AtomicU64::new(0),
            thread,
            thread_tx: weak_tx,
            limited,
        };
        let init =
            json!({ "clientInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") } });
        server.call("initialize", init).await?;
        server.notify("initialized");
        Ok(server)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn thread(&self) -> watch::Receiver<Option<Thread>> {
        self.thread.clone()
    }

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

    pub fn stop_shells(&self) -> usize {
        let Some(root) = self.process.here() else {
            return 0;
        };
        let snapshot = Snapshot::now();
        snapshot.kill_trees(snapshot.session_leaders_under(root))
    }

    pub fn stop(&self) {
        self.process.stop();
    }
}

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
                thread.send_replace(Some(Thread::new(started.thread.id, &started.thread.status)));
            }
        }
        Some("thread/status/changed") => {
            let Ok(changed) = serde_json::from_value::<ThreadStatusChangedNotification>(params)
            else {
                return;
            };
            thread.send_if_modified(|now| match now {
                Some(t) if t.id == changed.thread_id => {
                    *t = Thread::new(changed.thread_id, &changed.status);
                    true
                }
                _ => false,
            });
        }
        _ => {}
    }
}

fn is_usage_limit(msg: &Value) -> bool {
    msg["method"] == "error"
        && msg["params"]["willRetry"] != true
        && msg["params"]["error"]["codexErrorInfo"] == "usageLimitExceeded"
}
