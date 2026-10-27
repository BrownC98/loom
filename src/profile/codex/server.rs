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
    CommandExecutionRequestApprovalParams, FileChangeRequestApprovalParams, RequestId,
    ServerRequestResolvedNotification, ThreadActiveFlag, ThreadItem, ThreadReadParams,
    ThreadReadResponse, ThreadStartedNotification, ThreadStatus, ThreadStatusChangedNotification,
    Turn,
};
use crate::process_tree::{self, Snapshot};
use crate::profile::adapter::Chosen;
use crate::profile::process::Served;
use crate::protocol::{Session, WaitKind};
use crate::wsl::Inside;

const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const REPLAY_WAIT: Duration = Duration::from_secs(2);

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

#[derive(Clone, Debug)]
pub struct Approval {
    pub id: RequestId,
    pub asked: Asked,
}

#[derive(Clone, Debug)]
pub enum Asked {
    Command(CommandExecutionRequestApprovalParams),
    FileChange(FileChangeRequestApprovalParams),
}

type Changes = Mutex<HashMap<String, Vec<String>>>;

impl Approval {
    fn from_request(method: &str, id: Value, params: Value) -> Option<Self> {
        let asked = match method {
            "item/commandExecution/requestApproval" => {
                Asked::Command(serde_json::from_value(params).ok()?)
            }
            "item/fileChange/requestApproval" => {
                Asked::FileChange(serde_json::from_value(params).ok()?)
            }
            _ => return None,
        };
        Some(Self {
            id: serde_json::from_value(id).ok()?,
            asked,
        })
    }

    pub fn thread(&self) -> &str {
        match &self.asked {
            Asked::Command(asked) => &asked.thread_id,
            Asked::FileChange(asked) => &asked.thread_id,
        }
    }

    pub fn item(&self) -> &str {
        match &self.asked {
            Asked::Command(asked) => &asked.item_id,
            Asked::FileChange(asked) => &asked.item_id,
        }
    }

    fn is(&self, id: &RequestId) -> bool {
        match (&self.id, id) {
            (RequestId::Int64(a), RequestId::Int64(b)) => a == b,
            (RequestId::String(a), RequestId::String(b)) => a == b,
            _ => false,
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
    approvals: watch::Receiver<Vec<Approval>>,
    changes: Arc<Changes>,
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

    pub fn approvals(&self) -> watch::Receiver<Vec<Approval>> {
        self.approvals.clone()
    }

    pub async fn subscribe(&self, thread: &str) -> Result<(), String> {
        let params = json!({ "threadId": thread, "excludeTurns": true });
        self.call("thread/resume", params).await.map(drop)
    }

    pub async fn files(&self, approval: &Approval) -> Vec<String> {
        let Asked::FileChange(asked) = &approval.asked else {
            return Vec::new();
        };
        if let Some(files) = self.changes.lock().unwrap().get(&asked.item_id) {
            return files.clone();
        }
        let files = match self.active_turn(&asked.thread_id).await {
            Ok(turn) => turn
                .into_iter()
                .flat_map(|t| t.items)
                .find_map(|item| match item {
                    ThreadItem::FileChange { id, changes, .. } if id == asked.item_id => {
                        Some(changes.into_iter().map(|c| c.path).collect::<Vec<_>>())
                    }
                    _ => None,
                }),
            Err(e) => {
                eprintln!(
                    "loom: Codex 스레드 {}의 진행 중인 턴을 읽지 못했다: {e}",
                    asked.thread_id
                );
                None
            }
        };
        let files = files.unwrap_or_default();
        self.changes
            .lock()
            .unwrap()
            .insert(asked.item_id.clone(), files.clone());
        files
    }

    async fn active_turn(&self, thread: &str) -> Result<Option<Turn>, String> {
        let params = json!({ "threadId": thread, "excludeTurns": false });
        let resumed = self.call("thread/resume", params).await?;
        let turns: Vec<Turn> = serde_json::from_value(resumed["thread"]["turns"].clone())
            .map_err(|e| format!("thread/resume: {e}"))?;
        Ok(turns.into_iter().last())
    }

    pub async fn answer(
        &self,
        thread: &str,
        asked: impl Fn(&Approval) -> bool,
        result: Value,
    ) -> Result<Chosen, String> {
        self.subscribe(thread).await?;
        let mut approvals = self.approvals();
        let replayed = tokio::time::timeout(
            REPLAY_WAIT,
            approvals.wait_for(|all| all.iter().any(&asked)),
        )
        .await;
        let Some(id) = replayed
            .ok()
            .and_then(Result::ok)
            .and_then(|all| all.iter().find(|a| asked(a)).map(|a| a.id.clone()))
        else {
            return Ok(Chosen::PromptChanged);
        };
        let reply = json!({ "id": id, "result": result });
        self.to_server
            .send(Message::Text(reply.to_string().into()))
            .map_err(|_| "app-server connection closed".to_owned())?;
        let resolved = tokio::time::timeout(
            REPLY_TIMEOUT,
            approvals.wait_for(|all| !all.iter().any(|a| a.is(&id))),
        )
        .await;
        match resolved {
            Ok(Ok(_)) => Ok(Chosen::Sent),
            Ok(Err(_)) => Err("app-server connection closed".into()),
            Err(_) => Err(format!("approval {id}: not resolved")),
        }
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

    pub fn process(&self) -> &Served {
        &self.process
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
        let (approvals_tx, approvals) = watch::channel(Vec::new());
        let changes = Arc::new(Changes::default());
        let seen_changes = changes.clone();
        let replies = pending.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(text) = msg
                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                {
                    if is_usage_limit(&value) {
                        limited_tx.send_modify(|n| *n += 1);
                    }
                    receive(value, &replies, &thread_tx, &approvals_tx, &seen_changes);
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
            approvals,
            changes,
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

impl Drop for CodexServer {
    fn drop(&mut self) {
        let _ = self.to_server.send(Message::Close(None));
    }
}

fn receive(
    msg: Value,
    pending: &Pending,
    thread: &watch::Sender<Option<Thread>>,
    approvals: &watch::Sender<Vec<Approval>>,
    changes: &Changes,
) {
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
    if let (Some(method), Some(id)) = (msg["method"].as_str(), msg.get("id")) {
        if let Some(approval) = Approval::from_request(method, id.clone(), params) {
            approvals.send_if_modified(|all| {
                let new = !all.iter().any(|a| a.is(&approval.id));
                if new {
                    all.push(approval);
                }
                new
            });
        }
        return;
    }
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
        Some("item/started") => {
            if let Ok(ThreadItem::FileChange {
                id, changes: files, ..
            }) = serde_json::from_value(params["item"].clone())
            {
                let files = files.into_iter().map(|f| f.path).collect();
                changes.lock().unwrap().insert(id, files);
            }
        }
        Some("item/completed") => {
            if let Some(id) = params["item"]["id"].as_str() {
                changes.lock().unwrap().remove(id);
            }
        }
        Some("serverRequest/resolved") => {
            let Ok(resolved) = serde_json::from_value::<ServerRequestResolvedNotification>(params)
            else {
                return;
            };
            approvals.send_if_modified(|all| {
                let before = all.len();
                all.retain(|a| !a.is(&resolved.request_id));
                all.len() != before
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
