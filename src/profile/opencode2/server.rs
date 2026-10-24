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
use crate::protocol::PermissionReply;
use crate::wsl::Inside;

const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL: Duration = Duration::from_millis(50);
const USER: &str = "opencode";
const SSE_DATA: &str = "data: ";

pub struct Server {
    url: String,
    password: String,
    auth: String,
    pid: Option<u32>,
    stopped: Arc<watch::Sender<bool>>,
}

impl Server {
    pub async fn start(
        program: &str,
        cwd: &Path,
        inside: Option<&Inside>,
        node_env: &[(&'static str, OsString)],
        password: String,
    ) -> Result<Self, String> {
        let port = process_tree::free_port()?;
        let port_text = port.to_string();
        let args = ["serve", "--hostname", "127.0.0.1", "--port", &port_text];
        let mut env = node_env.to_vec();
        env.push((super::PASSWORD_ENV, password.clone().into()));
        let mut cmd = match inside {
            Some(inside) => inside.server(&[&[program][..], &args[..]].concat(), &env),
            None => {
                let mut cmd = process_tree::command(program);
                cmd.args(args)
                    .current_dir(cwd)
                    .envs(env.iter().map(|(k, v)| (k, v)));
                cmd
            }
        };
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
        tokio::spawn(async move { child.wait().await });
        let server = Self::connect(format!("http://127.0.0.1:{port}"), password, pid);
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

    pub fn stopped(&self) -> watch::Receiver<bool> {
        self.stopped.subscribe()
    }

    pub fn follow_events(&self, mut on_event: impl FnMut(Value) + Send + 'static) {
        let url = format!("{}/api/event", self.url);
        let auth = self.auth.clone();
        let stopped = self.stopped.clone();
        std::thread::spawn(move || {
            let _ended = Ended(stopped);
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

    pub async fn create_session(&self, cwd: &Path) -> Result<String, String> {
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

    pub async fn reply_permission(
        &self,
        session: &str,
        request: &str,
        reply: PermissionReply,
    ) -> Result<(), String> {
        let path = format!("/api/session/{session}/permission/{request}/reply");
        self.call("POST", &path, Some(json!({ "decision": reply })))
            .await
            .map(drop)
    }

    pub async fn interrupt(&self, session: &str) -> Result<(), String> {
        let path = format!("/api/session/{session}/interrupt");
        self.call("POST", &path, None).await.map(drop)
    }

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

    pub fn stop(&self) {
        if let Some(root) = self.pid {
            Snapshot::now().kill_trees([root]);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[tokio::test]
    async fn other_errors_keep_the_old_message_shape() {
        let (url, _) = fake_server(&[(401, r#"{"code":"unauthorized"}"#)]);
        let server = Server::connect(url, "pw".into(), None);
        let err = server.create_session(Path::new(".")).await.unwrap_err();
        assert_eq!(err, r#"POST /api/session: 401 {"code":"unauthorized"}"#);
    }
}
