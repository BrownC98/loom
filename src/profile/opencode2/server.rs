use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use crate::process_tree::{self, Snapshot};
use crate::profile::process::Served;
use crate::wsl::Inside;

const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const V1_START_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(50);
const SSE_DATA: &str = "data: ";

#[derive(Clone, Copy, PartialEq)]
pub enum Api {
    V2,
    MiMo,
    Kilo,
}

impl Api {
    pub fn password_env(self) -> &'static str {
        match self {
            Self::V2 => super::PASSWORD_ENV,
            Self::MiMo => "MIMOCODE_SERVER_PASSWORD",
            Self::Kilo => "KILO_SERVER_PASSWORD",
        }
    }

    pub(super) fn username_env(self) -> Option<&'static str> {
        match self {
            Self::V2 => None,
            Self::MiMo => Some("MIMOCODE_SERVER_USERNAME"),
            Self::Kilo => Some("KILO_SERVER_USERNAME"),
        }
    }

    pub fn config_env(self) -> &'static str {
        match self {
            Self::V2 => super::CONFIG_ENV,
            Self::MiMo => "MIMOCODE_CONFIG_CONTENT",
            Self::Kilo => "KILO_CONFIG_CONTENT",
        }
    }

    pub(super) fn user(self) -> &'static str {
        match self {
            Self::V2 => "opencode",
            Self::MiMo => "mimocode",
            Self::Kilo => "kilo",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::V2 => "/api",
            Self::MiMo | Self::Kilo => "",
        }
    }
}

pub struct Server {
    api: Api,
    url: String,
    password: String,
    auth: String,
    process: Served,
    stopped: Arc<watch::Sender<bool>>,
}

impl Server {
    pub async fn start(
        api: Api,
        program: &str,
        cwd: &Path,
        inside: Option<&Inside>,
        node_env: &[(&'static str, OsString)],
        password: String,
    ) -> Result<Self, String> {
        let port = process_tree::free_port()?;
        let port_text = port.to_string();
        let argv = [
            program,
            "serve",
            "--hostname",
            "127.0.0.1",
            "--port",
            &port_text,
        ]
        .map(str::to_owned);
        let mut env = node_env.to_vec();
        env.push((api.password_env(), password.clone().into()));
        if let Some(key) = api.username_env() {
            env.push((key, api.user().into()));
        }
        let at = process_tree::Listen::Port(port);
        let limit = match api {
            Api::V2 => REPLY_TIMEOUT,
            Api::MiMo | Api::Kilo => V1_START_TIMEOUT,
        };
        let process = Served::start(&argv, cwd, inside, &env, &at, limit).await?;
        let server = Self::connect(api, format!("http://127.0.0.1:{port}"), password, process);
        if let Err(e) = server.ready().await {
            server.stop();
            return Err(e);
        }
        Ok(server)
    }

    async fn ready(&self) -> Result<(), String> {
        let info = match self.api {
            Api::V2 => format!("{}/api/info", self.url),
            Api::MiMo | Api::Kilo => format!("{}/global/health", self.url),
        };
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

    pub fn connect(api: Api, url: String, password: String, process: Served) -> Self {
        let basic =
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{password}", api.user()));
        Self {
            api,
            url,
            password,
            auth: format!("Basic {basic}"),
            process,
            stopped: Arc::new(watch::channel(false).0),
        }
    }

    pub fn api(&self) -> Api {
        self.api
    }

    pub fn process(&self) -> &Served {
        &self.process
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    pub fn stopped(&self) -> watch::Receiver<bool> {
        self.stopped.subscribe()
    }

    pub async fn follow_events(
        &self,
        mut on_event: impl FnMut(Value) + Send + 'static,
    ) -> Result<(), String> {
        let (ready, started) = oneshot::channel();
        let url = format!("{}{}/event", self.url, self.api.prefix());
        let auth = self.auth.clone();
        let stopped = self.stopped.clone();
        std::thread::spawn(move || {
            let _ended = Ended(stopped);
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(REPLY_TIMEOUT))
                .timeout_recv_response(Some(REPLY_TIMEOUT))
                .build()
                .into();
            let Ok(mut res) = agent.get(&url).header("authorization", &auth).call() else {
                return;
            };
            if ready.send(()).is_err() {
                return;
            }
            let lines =
                std::io::BufRead::lines(std::io::BufReader::new(res.body_mut().as_reader()));
            for line in lines.map_while(Result::ok) {
                if let Some(Ok(event)) = line.strip_prefix(SSE_DATA).map(serde_json::from_str) {
                    on_event(event);
                }
            }
        });
        tokio::time::timeout(REPLY_TIMEOUT, started)
            .await
            .map_err(|_| "event subscription timed out".to_owned())?
            .map_err(|_| "event subscription failed".to_owned())
    }

    pub async fn create_session(&self, cwd: &Path) -> Result<String, String> {
        let body = match self.api {
            Api::V2 => json!({ "location": { "directory": cwd } }),
            Api::MiMo | Api::Kilo => json!({}),
        };
        let reply = self.call("POST", "/session", Some(body)).await?;
        let data = match self.api {
            Api::V2 => &reply["data"],
            Api::MiMo | Api::Kilo => &reply,
        };
        data["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("session create: unknown reply: {reply}"))
    }

    pub async fn set_permissions(&self, session: &str, rules: &[Value]) -> Result<(), String> {
        let path = format!("/session/{session}");
        self.call("PATCH", &path, Some(json!({ "permissions": rules })))
            .await
            .map(drop)
    }

    pub async fn put_instructions(
        &self,
        session: &str,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let path = format!("/experimental/session/{session}/instructions/entries/{key}");
        self.call("PUT", &path, Some(json!({ "value": value })))
            .await
            .map(drop)
    }

    pub async fn prompt(
        &self,
        session: &str,
        id: &str,
        text: &str,
        delivery: &str,
        model: &crate::protocol::Model,
    ) -> Result<(), String> {
        let (path, body) = match self.api {
            Api::V2 => (
                format!("/session/{session}/prompt"),
                json!({ "id": id, "text": text, "delivery": delivery }),
            ),
            Api::MiMo | Api::Kilo => {
                let suffix = id.strip_prefix("msg_").unwrap_or(id);
                let messages = self
                    .call("GET", &format!("/session/{session}/message"), None)
                    .await?;
                if messages.as_array().is_some_and(|messages| {
                    messages.iter().any(|m| {
                        m["info"]["id"]
                            .as_str()
                            .is_some_and(|i| i.ends_with(suffix))
                    })
                }) {
                    return Ok(());
                }
                let id = super::v1::message_id(self.api, id)?;
                let mut body =
                    json!({ "messageID": id, "parts": [{ "type": "text", "text": text }] });
                if let Some((provider, name)) =
                    model.model.as_deref().and_then(|m| m.split_once('/'))
                {
                    body["model"] = json!({ "providerID": provider, "modelID": name });
                }
                if let Some(variant) = &model.effort {
                    body["variant"] = json!(variant);
                }
                (format!("/session/{session}/prompt_async"), body)
            }
        };
        self.call("POST", &path, Some(body)).await.map(drop)
    }

    pub async fn permissions(&self, session: &str) -> Result<Value, String> {
        let path = match self.api {
            Api::V2 => format!("/session/{session}/permission"),
            Api::MiMo | Api::Kilo => "/permission".to_owned(),
        };
        self.call("GET", &path, None).await
    }

    pub async fn reply_permission(
        &self,
        session: &str,
        request: &str,
        reply: super::PermissionReply,
    ) -> Result<(), String> {
        let (path, body) = match self.api {
            Api::V2 => (
                format!("/session/{session}/permission/{request}/reply"),
                json!({ "decision": reply }),
            ),
            Api::MiMo | Api::Kilo => (
                format!("/permission/{request}/reply"),
                json!({ "reply": reply }),
            ),
        };
        self.call("POST", &path, Some(body)).await.map(drop)
    }

    pub async fn interrupt(&self, session: &str) -> Result<(), String> {
        let action = match self.api {
            Api::V2 => "interrupt",
            Api::MiMo | Api::Kilo => "abort",
        };
        let path = format!("/session/{session}/{action}");
        self.call("POST", &path, None).await.map(drop)
    }

    pub async fn stop_shells(&self, session: &str) {
        if self.api != Api::V2 {
            return;
        }
        let Some(root) = self.process.here() else {
            return;
        };
        let shells = match self.call("GET", "/shell", None).await {
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

    pub fn stop(&self) {
        self.process.stop();
    }

    async fn call(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let url = format!("{}{prefix}{path}", self.url, prefix = self.api.prefix());
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

struct Reply {
    status: u16,
    text: String,
}

impl Reply {
    fn json(self) -> Result<Value, String> {
        if !(200..300).contains(&self.status) {
            return Err(self.refusal());
        }
        if self.text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&self.text).map_err(|e| format!("unknown reply: {e}"))
    }

    fn refusal(&self) -> String {
        format!("{} {}", self.status, self.text)
    }
}

struct Ended(Arc<watch::Sender<bool>>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
