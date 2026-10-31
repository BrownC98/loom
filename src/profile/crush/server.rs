use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::{Conversation, HOOK};
use crate::place::Files;
use crate::process_tree::{self, Listen};
use crate::profile::adapter::{LaunchAt, Port, Record, Started};
use crate::profile::process::Served;
use crate::protocol::{
    AdapterRecord, Delivery, EndedTurn, IdleReply, IdleRequest, Kind, Model, ModelChoice, Report,
    TakeRequest,
};

const HTTP_WAIT: Duration = Duration::from_secs(20);
const CANCEL_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Serialize, Deserialize)]
struct State {
    url: String,
    client: String,
}

#[derive(Clone)]
pub struct Server {
    process: Served,
    state: State,
    conversation: Conversation,
    workspace: String,
}

impl Server {
    pub async fn start(
        at: &LaunchAt<'_>,
        program: &[String],
        resume: Option<Conversation>,
        rules: &str,
        bypass: &[String],
    ) -> Result<(Self, Started), String> {
        let data = match &resume {
            Some(conversation) => conversation.data.clone(),
            None => {
                let name = format!("crush-{}", random_id()?);
                match at.inside {
                    Some(inside) => format!("{}/{name}", inside.run_dir()),
                    None => at.run_dir.join(name).display().to_string(),
                }
            }
        };
        let local = at.site.local(&data);
        let files = Files::of(at.inside);
        files
            .write(&local.join("loom.md"), rules.as_bytes())
            .map_err(|e| e.to_string())?;
        let config = local.join("crush.json");
        let mut settings: Value = match files.read(&config) {
            Ok(text) => serde_json::from_slice(&text).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.to_string()),
        };
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            settings["hooks"][event] = json!([{"command": HOOK, "timeout": 10}]);
        }
        settings["options"]["skills_paths"] = json!([at.skills]);
        files
            .write(&config, settings.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        let port = process_tree::free_port()?;
        let host = format!("tcp://127.0.0.1:{port}");
        let mut argv = program.to_vec();
        argv.extend([
            "server".into(),
            "--host".into(),
            host,
            "--data-dir".into(),
            data.clone(),
        ]);
        let client = random_id()?;
        let process = Served::start(
            &argv,
            at.cwd,
            at.inside,
            at.env,
            &Listen::Port(port),
            HTTP_WAIT,
        )
        .await?;
        let mut server = Self {
            process,
            state: State {
                url: format!("http://127.0.0.1:{port}/v1"),
                client,
            },
            conversation: Conversation {
                id: resume.as_ref().map(|c| c.id.clone()).unwrap_or_default(),
                data,
            },
            workspace: String::new(),
        };
        let workspace = json!({
                "client_id": server.state.client,
                "path": at.site.cli_cwd(at.cwd),
                "data_dir": server.conversation.data,
                "yolo": !bypass.is_empty(),
                "env": at.env.iter().map(|(k,v)| format!("{k}={}", v.to_string_lossy())).collect::<Vec<_>>(),
        });
        let choice = at.choice.clone();
        let mut started = Started::default();
        match resume {
            Some(_) => {
                let mut initializing = server.clone();
                started.before_start = Some(Box::pin(async move {
                    initializing.initialize(workspace, choice).await
                }));
            }
            None => server.initialize(workspace, choice).await?,
        }
        Ok((server, started))
    }

    async fn initialize(&mut self, workspace: Value, choice: ModelChoice) -> Result<(), String> {
        let initialized = async {
            let workspace = self.call("POST", "/workspaces", Some(workspace)).await?;
            self.workspace = field(&workspace, "id")?;
            let config = self.workspace("GET", "/config", None).await?;
            let mut context: Vec<String> =
                serde_json::from_value(config["options"]["context_paths"].clone())
                    .unwrap_or_default();
            let rules = format!("{}/loom.md", self.conversation.data);
            if !context.contains(&rules) {
                context.push(rules);
            }
            self.workspace(
                "POST",
                "/config/set",
                Some(json!({
                    "scope": 1, "key": "options.context_paths", "value": context,
                })),
            )
            .await?;
            if !choice.is_empty() {
                let mut model = config["models"]["large"].clone();
                if let Some(selected) = &choice.model {
                    match selected.split_once('/') {
                        Some((provider, id)) => {
                            model["provider"] = provider.into();
                            model["model"] = id.into();
                        }
                        None => model["model"] = selected.clone().into(),
                    }
                }
                if let Some(effort) = &choice.effort {
                    model["reasoning_effort"] = effort.clone().into();
                }
                self.workspace(
                    "POST",
                    "/config/model",
                    Some(json!({
                        "scope": 1, "model_type": "large", "model": model,
                    })),
                )
                .await?;
            }
            self.workspace("POST", "/agent/init", Some(json!({"interactive": true})))
                .await?;
            if self.conversation.id.is_empty() {
                self.conversation.id = field(
                    &self
                        .workspace("POST", "/sessions", Some(json!({"title": "loom"})))
                        .await?,
                    "id",
                )?;
            } else {
                self.workspace("GET", &format!("/sessions/{}", self.conversation.id), None)
                    .await?;
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(e) = initialized {
            self.process.stop();
            return Err(e);
        }
        Ok(())
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn host(&self) -> String {
        self.state
            .url
            .trim_end_matches("/v1")
            .replacen("http://", "tcp://", 1)
    }

    pub fn record(&self) -> AdapterRecord {
        self.process.record(&self.state)
    }

    pub fn stored(rec: &Record) -> Option<Self> {
        let (process, state) = Served::stored(rec)?;
        let conversation = serde_json::from_str(&rec.session.as_ref()?.0).ok()?;
        Some(Self {
            process,
            state,
            conversation,
            workspace: String::new(),
        })
    }

    async fn find_workspace(&mut self) -> Result<(), String> {
        if !self.workspace.is_empty() {
            return Ok(());
        }
        let workspaces = self.call("GET", "/workspaces", None).await?;
        let workspace = workspaces
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|w| w["data_dir"] == self.conversation.data)
            })
            .ok_or("Crush 전용 워크스페이스가 없다")?;
        self.workspace = field(workspace, "id")?;
        Ok(())
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let (method, url) = (method.to_owned(), format!("{}{path}", self.state.url));
        tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(HTTP_WAIT))
                .build()
                .into();
            let mut request = ureq::http::Request::builder()
                .method(method.as_str())
                .uri(url);
            if body.is_some() {
                request = request.header("content-type", "application/json");
            }
            let request = request
                .body(body.map(|b| b.to_string()).unwrap_or_default())
                .map_err(|e| e.to_string())?;
            let mut response = agent.run(request).map_err(|e| e.to_string())?;
            let text = response
                .body_mut()
                .read_to_string()
                .map_err(|e| e.to_string())?;
            if text.is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_str(&text).map_err(|e| e.to_string())
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn workspace(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        self.call(
            method,
            &format!("/workspaces/{}{path}", self.workspace),
            body,
        )
        .await
    }

    async fn prompt(&self, text: &str) -> Result<(), String> {
        self.workspace(
            "POST",
            "/agent",
            Some(json!({
                "session_id": self.conversation.id,
                "prompt": text,
                "run_id": random_id()?,
                "hidden_user_message": true,
            })),
        )
        .await
        .map(drop)
    }

    async fn cancel(&self) -> Result<(), String> {
        self.workspace(
            "POST",
            &format!("/agent/sessions/{}/cancel", self.conversation.id),
            Some(json!({})),
        )
        .await
        .map(drop)
    }

    async fn events(&self) -> Result<mpsc::UnboundedReceiver<Event>, String> {
        let (sent, events) = mpsc::unbounded_channel();
        let (ready, connected) = oneshot::channel();
        let url = format!(
            "{}/workspaces/{}/events?client_id={}",
            self.state.url, self.workspace, self.state.client
        );
        let session = self.conversation.id.clone();
        std::thread::spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(HTTP_WAIT))
                .build()
                .into();
            let mut response = match agent.get(&url).call() {
                Ok(response) => response,
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            };
            if ready.send(Ok(())).is_err() {
                return;
            }
            let lines =
                std::io::BufRead::lines(std::io::BufReader::new(response.body_mut().as_reader()));
            for line in lines.map_while(Result::ok) {
                let Some(text) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(text) else {
                    continue;
                };
                if let Some(event) = event(&value, &session)
                    && sent.send(event).is_err()
                {
                    break;
                }
            }
        });
        tokio::time::timeout(HTTP_WAIT, connected)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())??;
        Ok(events)
    }
}

fn field(value: &Value, name: &str) -> Result<String, String> {
    value[name]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("Crush 응답에 {name} 값이 없다"))
}

fn random_id() -> Result<String, String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    let n = u128::from_be_bytes(bytes);
    let text = format!("{n:032x}");
    Ok(format!(
        "{}-{}-4{}-a{}-{}",
        &text[..8],
        &text[8..12],
        &text[13..16],
        &text[17..20],
        &text[20..]
    ))
}

enum Event {
    Started {
        prompt: String,
    },
    ModelChanged,
    Finished {
        reply: Option<String>,
        stopped: bool,
    },
}

fn event(value: &Value, session: &str) -> Option<Event> {
    let data = &value["payload"]["payload"];
    if value["type"] == "config_changed" {
        return Some(Event::ModelChanged);
    }
    if data["session_id"] != session {
        return None;
    }
    match value["type"].as_str()? {
        "run_complete" => Some(Event::Finished {
            reply: data["text"].as_str().map(str::to_owned),
            stopped: data["cancelled"] == true
                || data["error"].as_str().is_some_and(|s| !s.is_empty()),
        }),
        "message" if value["payload"]["type"] == "created" => match data["role"].as_str()? {
            "user" => Some(Event::Started {
                prompt: data["parts"]
                    .as_array()?
                    .iter()
                    .filter(|part| part["type"] == "text" && part["data"]["hidden"] != true)
                    .filter_map(|part| part["data"]["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            "assistant" => Some(Event::ModelChanged),
            _ => None,
        },
        _ => None,
    }
}

async fn pending<T>(future: &mut Option<BoxFuture<'_, T>>) -> T {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

pub async fn drive(mut server: Server, port: Arc<dyn Port>, brief: String) {
    let result = tokio::select! {
        biased;
        () = port.ended() => Ok(()),
        result = async {
            port.ready().await?;
            server.find_workspace().await?;
            run(&server, &*port, brief).await
        } => result,
    };
    if let Err(error) = result {
        crate::log::logln!("loom: Crush 연동을 멈췄다: {error}");
        server.process.stop();
        port.report(Report::Lost);
        port.ended().await;
    }
    server.process.stop();
}

async fn run(server: &Server, port: &dyn Port, brief: String) -> Result<(), String> {
    let mut events = server.events().await?;
    let state = server
        .workspace(
            "GET",
            &format!("/agent/sessions/{}", server.conversation.id),
            None,
        )
        .await?;
    let mut active = state["is_busy"] == true;
    if !brief.trim().is_empty() {
        server.prompt(&brief).await?;
        active = true;
    }
    let mut idle = None;
    let mut turn_end = None;
    let mut interrupted: Option<(Vec<Delivery>, tokio::time::Instant)> = None;
    loop {
        if !active && idle.is_none() {
            idle = Some(port.idle(IdleRequest {
                turn_end: turn_end.take(),
                ..IdleRequest::default()
            }));
        }
        let cancel_deadline = interrupted.as_ref().map(|(_, deadline)| *deadline);
        tokio::select! {
            biased;
            () = async {
                match cancel_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                let cause = "Crush가 중단 뒤 턴을 끝내지 않았다".to_owned();
                if let Some((mail, _)) = interrupted.take() {
                    port.report(Report::Refused {
                        ids: mail.iter().map(|m| m.id).collect(),
                        cause: cause.clone(),
                    });
                }
                return Err(cause);
            },
            event = events.recv() => match event.ok_or("Crush 사건 연결이 끝났다")? {
                Event::Started { prompt } => {
                    port.take(TakeRequest {
                        step: "started".into(),
                        kinds: Vec::new(),
                        limit_bytes: 0,
                        session: None,
                        human_input: true,
                        prompt: Some(prompt),
                        turn: None,
                        prompt_id: None,
                    }).map_err(|e| e.to_string())?;
                    active = true;
                    if let Some(future) = idle.take()
                        && let Some(Ok(reply)) = future.now_or_never()
                    {
                        match reply {
                            IdleReply::Mail(mail) => port.release(&mail.iter().map(|m| m.id).collect::<Vec<_>>()).map_err(|e| e.to_string())?,
                            IdleReply::Nudge { nudge } => server.prompt(&nudge).await?,
                        }
                    }
                    port.report(Report::Busy);
                }
                Event::ModelChanged => {
                    let config = server.workspace("GET", "/config", None).await?;
                    let model = &config["models"]["large"];
                    port.report(Report::Model(Model {
                        model: Some(format!("{}/{}", field(model, "provider")?, field(model, "model")?)),
                        effort: model["reasoning_effort"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
                    }));
                }
                Event::Finished { reply, stopped } => {
                    active = false;
                    if let Some((mail, _)) = interrupted.take() {
                        deliver(server, port, &mail).await?;
                        active = true;
                    } else if stopped {
                        port.report(Report::TurnStopped { turn: None });
                    } else {
                        turn_end = Some(EndedTurn { reply, turn: None });
                    }
                }
            },
            result = pending(&mut idle) => {
                idle = None;
                match result? {
                    IdleReply::Mail(mail) if mail.is_empty() => {
                        active = true;
                        continue;
                    }
                    IdleReply::Mail(mail) => deliver(server, port, &mail).await?,
                    IdleReply::Nudge { nudge } => server.prompt(&nudge).await?,
                }
                active = true;
            }
            mail = port.next_mail(&[Kind::Interrupt]), if active && interrupted.is_none() => {
                let mail = mail.map_err(|e| e.to_string())?;
                if let Err(error) = server.cancel().await {
                    port.release(&mail.iter().map(|m| m.id).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
                    return Err(error);
                }
                interrupted = Some((mail, tokio::time::Instant::now() + CANCEL_WAIT));
            }
        }
    }
}

async fn deliver(server: &Server, port: &dyn Port, mail: &[Delivery]) -> Result<(), String> {
    let ids: Vec<_> = mail.iter().map(|m| m.id).collect();
    if let Err(cause) = server.prompt(&crate::profile::message_text(mail)).await {
        port.report(Report::Refused {
            ids,
            cause: cause.clone(),
        });
        return Err(cause);
    }
    port.ack(&ids).map_err(|e| e.to_string())
}
