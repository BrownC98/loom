//! OpenCode 2 프로필: 노드마다 `opencode serve` 하나를 띄우고, 메일함 메시지를 세션 입력함(`/api/session/{id}/prompt`)에 넣는다.
//! TUI는 `--server <주소> --session <id>`로 같은 세션에 붙는다. 바뀌는 이유는 OpenCode 2 API다.
//! 서버 프로세스와 HTTP 연결은 `server.rs`에 둔다. 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

mod server;

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::graph::{Delivery, Kind, Model};
use crate::skills::Launch;
use crate::wsl::Inside;

use super::{Mailbox, VersionMismatch};
use server::Server;

/// 실측과 `docs/design.md`의 API 근거를 본 OpenCode 버전.
pub const PINNED_VERSION: &str = "2.0.32";
/// TUI와 서버가 같은 값을 읽는 비밀번호 환경 변수. 서버는 HTTP Basic(`opencode:<값>`)만 받는다.
pub const PASSWORD_ENV: &str = "OPENCODE_PASSWORD";
/// 카탈로그 `opencode2` 항목의 실행 파일.
pub const CMD: &str = "opencode2";
/// 같은 메시지 id로 다시 넣는 횟수와 간격. 서버는 같은 id를 한 번만 넣는다(실측). 그래서 다시 넣어도 두 번 들어가지 않는다.
const PROMPT_ATTEMPTS: u32 = 5;
const PROMPT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// 서버가 사람의 설정 위에 합치는 설정 글(JSON). 근거: opencode 2.0.32 `OPENCODE_CONFIG_CONTENT`.
const CONFIG_ENV: &str = "OPENCODE_CONFIG_CONTENT";

/// 세션 지시 항목(`instructions/entries/loom`). 모델이 모든 턴에 본다(실측).
/// 사용법 끝에 붙는 대기 방식. 바꾸면 뜬 노드가 `rules-changed`를 받는다(`profile::usage_digest`).
pub(super) const WAITING: &str = "When waiting for children and no independent work remains, end your turn. Their messages arrive as new input automatically. Do not run an unbounded loom wait.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

/// `opencode` TUI 시작 인자. 카탈로그의 `--standalone`(비공개 서버)은 `--server`와 함께 쓸 수 없다.
pub fn args(url: &str, session: &str) -> Vec<String> {
    vec![
        "--server".into(),
        url.into(),
        "--session".into(),
        session.into(),
    ]
}

/// `opencode2 --version` 출력(예: `opencode v2.0.32`)이 고정 버전과 다르면 화면에 보일 경고다.
pub fn version_warning(version_output: &str) -> Option<VersionMismatch> {
    let last = version_output.split_whitespace().last()?;
    let installed = last.strip_prefix('v').unwrap_or(last);
    (installed != PINNED_VERSION).then(|| VersionMismatch {
        installed: installed.to_owned(),
        pinned: PINNED_VERSION.to_owned(),
    })
}

/// `session.model.selected` 이벤트의 모델(`Model.Ref`). variant가 없으면 기본 variant다.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelRef {
    #[serde(rename = "providerID")]
    provider_id: String,
    id: String,
    variant: Option<String>,
}

/// 서버 이벤트가 `session`의 모델 선택이면 그 모델이다. 모델 id는 `<공급자>/<모델>`로 쓴다(TUI `--model` 형식).
fn selected_model(event: &Value, session: &str) -> Option<Model> {
    let data = &event["data"];
    if event["type"] != "session.model.selected" || data["sessionID"] != session {
        return None;
    }
    let model = ModelRef::deserialize(&data["model"]).ok()?;
    Some(Model {
        model: Some(format!("{}/{}", model.provider_id, model.id)),
        effort: model.variant,
    })
}

/// 세션 이벤트 가운데 턴 끝 질문 검사가 보는 것(2.0.32 실측: `session.execution.started` → `session.text.ended` → `session.execution.succeeded`).
#[derive(Debug, PartialEq)]
enum TurnEvent {
    Started,
    /// 답 글 하나를 다 냈다. 한 턴에 여럿이면 마지막이 턴을 끝낸 답이다.
    Said(String),
    /// 턴이 끝났다. 실패·중단으로 끝난 턴은 검사하지 않는다.
    Succeeded,
    /// 턴이 실패나 중단으로 끝났다.
    Stopped,
    /// 사용자 입력 하나가 입력함에 들어왔다. id가 loom이 넣은 것이 아니면 사람이 TUI에 친 글이다.
    Enqueued {
        id: String,
    },
}

/// `run`이 받는 세션 사건.
enum Turn {
    /// 턴이 끝났다. 그 턴의 마지막 답 글.
    Ended(Option<String>),
    /// 사람이 TUI에 입력했다.
    HumanInput,
}

fn turn_event(event: &Value, session: &str) -> Option<TurnEvent> {
    let data = &event["data"];
    if data["sessionID"] != session {
        return None;
    }
    match event["type"].as_str()? {
        "session.execution.started" => Some(TurnEvent::Started),
        "session.text.ended" => Some(TurnEvent::Said(data["text"].as_str()?.to_owned())),
        "session.execution.succeeded" => Some(TurnEvent::Succeeded),
        "session.execution.failed" | "session.execution.interrupted" => Some(TurnEvent::Stopped),
        "session.inbox.enqueued" if data["item"]["type"] == "user" => Some(TurnEvent::Enqueued {
            id: data["inboxID"].as_str()?.to_owned(),
        }),
        _ => None,
    }
}

/// 노드 하나의 서버와 세션. 데몬이 노드를 띄우기 전에 만든다.
pub struct Node {
    server: Server,
    session: String,
    model: watch::Receiver<Model>,
    /// 세션이 턴 중이다. 다시 띄우기가 턴 끝을 기다린다. 다시 붙은 데몬은 다음 턴 사건까지 쉰다고 본다.
    working: watch::Receiver<bool>,
    /// 세션 사건. `run`이 한 번 가져간다.
    turns: Mutex<Option<mpsc::UnboundedReceiver<Turn>>>,
    /// loom이 입력함에 넣은 메시지 id. 이벤트가 오면 지운다.
    sent: Arc<Mutex<HashSet<String>>>,
}

impl Node {
    /// 서버를 띄우고 `cwd`에 세션을 만든 뒤 사용법을 지시 항목으로 넣는다. `resume`이 있으면 만들지 않고 그 세션을 잇는다.
    /// 세션은 서버가 아니라 사람의 OpenCode 저장소에 남으므로 새 서버도 연다(다시 띄우기).
    /// `node_env`는 노드 프로세스와 같은 환경이다. 셸 명령은 이 서버 아래에서 돌므로 `loom`이 그 값을 읽는다.
    /// `password`는 노드마다 새로 만든 서버 비밀번호다.
    /// `skills`는 loom이 싣는 스킬 폴더다(`skills::Plugin::skills`). 설정 글 변수로 서버의 스킬 폴더에 더한다.
    /// 이 변수는 사람의 설정 파일을 덮지 않고 그 위에 합쳐진다.
    /// `inside`가 있으면 서버를 그 WSL 배포판에서 띄우고 세션 폴더도 배포판 안의 경로로 준다. `skills`는 서버가 보는 경로다.
    pub async fn start(
        cwd: &Path,
        inside: Option<&Inside>,
        node_env: &[(&'static str, OsString)],
        skills: &Path,
        password: String,
        launch: Launch,
        resume: Option<&str>,
    ) -> Result<Self, String> {
        let config = serde_json::json!({ "skills": [skills] }).to_string();
        let mut env = node_env.to_vec();
        env.push((CONFIG_ENV, config.into()));
        let server = Server::start(CMD, cwd, inside, &env, password).await?;
        let directory = inside.map_or(cwd, |inside| Path::new(inside.cwd()));
        let created = match resume {
            Some(session) => Ok(session.to_owned()),
            None => server.create_session(directory).await,
        };
        let session = match created {
            Ok(session) => session,
            Err(e) => {
                server.stop();
                return Err(e);
            }
        };
        if let Err(e) = server
            .put_instructions(&session, "loom", &usage(launch))
            .await
        {
            server.stop();
            return Err(e);
        }
        Ok(Self::follow(server, session))
    }

    /// 앞 데몬이 띄운 서버와 세션에 다시 붙는다. 모델은 TUI가 다음 입력을 보낼 때 다시 알린다.
    pub fn reconnect(url: String, pid: Option<u32>, session: String, password: String) -> Self {
        Self::follow(Server::connect(url, password, pid), session)
    }

    /// 다시 붙는 데 쓰는 값. DB에 남긴다.
    pub fn link(&self) -> super::ServerLink {
        super::ServerLink::OpenCode2 {
            url: self.server.url().to_owned(),
            pid: self.server.pid(),
            session: self.session.clone(),
            password: self.server.password().to_owned(),
        }
    }

    /// 서버 이벤트에서 세션의 모델을 읽기 시작한다.
    fn follow(server: Server, session: String) -> Self {
        let (tx, model) = watch::channel(Model::default());
        let (tx_working, working) = watch::channel(false);
        let (tx_turns, turns) = mpsc::unbounded_channel();
        let sent = Arc::new(Mutex::new(HashSet::new()));
        let ours = session.clone();
        let ours_sent = sent.clone();
        let mut said = None;
        server.follow_events(move |event| {
            if let Some(selected) = selected_model(&event, &ours) {
                tx.send_replace(selected);
            }
            match turn_event(&event, &ours) {
                Some(TurnEvent::Started) => {
                    said = None;
                    tx_working.send_replace(true);
                }
                Some(TurnEvent::Said(text)) => said = Some(text),
                Some(TurnEvent::Succeeded) => {
                    tx_working.send_replace(false);
                    let _ = tx_turns.send(Turn::Ended(said.take()));
                }
                Some(TurnEvent::Stopped) => {
                    tx_working.send_replace(false);
                }
                Some(TurnEvent::Enqueued { id }) if !ours_sent.lock().unwrap().remove(&id) => {
                    let _ = tx_turns.send(Turn::HumanInput);
                }
                Some(TurnEvent::Enqueued { .. }) | None => {}
            }
        });
        Self {
            server,
            session,
            model,
            working,
            turns: Mutex::new(Some(turns)),
            sent,
        }
    }

    /// 세션의 모델과 variant(추론 강도). TUI는 고른 값을 다음 입력을 보낼 때 서버에 알린다(실측).
    /// 그래서 첫 입력 전에는 없다. 서버가 끝나면 `changed()`가 실패한다.
    pub fn model(&self) -> watch::Receiver<Model> {
        self.model.clone()
    }

    /// 세션이 턴 중인지. 서버가 끝나면 `changed()`가 실패한다.
    pub fn working(&self) -> watch::Receiver<bool> {
        self.working.clone()
    }

    /// TUI가 붙을 서버와 세션.
    pub fn remote(&self) -> super::Remote<'_> {
        super::Remote::OpenCode2 {
            url: self.server.url(),
            session: &self.session,
            password: self.server.password(),
        }
    }

    /// 세션에서 돌고 있는 셸 명령과 그 자손을 끝낸다. 데몬이 중단 메시지를 넣기 전에 부른다.
    /// 실행만 멈추면 `setsid`로 띄운 셸 자손이 남는다(실측).
    pub async fn stop_shells(&self) {
        self.server.stop_shells(&self.session).await;
    }

    /// 서버와 그 아래 프로세스를 모두 끝낸다. `run`도 끝난다.
    pub fn stop(&self) {
        self.server.stop();
    }

    /// 메일함을 세션에 넘긴다. 서버가 끝나면 끝난다. 노드마다 이 작업 하나만 꺼내므로 같은 메시지를 두 번 넘기지 않는다.
    pub async fn run(&self, mailbox: impl Mailbox) {
        let api = Session {
            server: &self.server,
            id: &self.session,
            sent: &self.sent,
        };
        let mut stopped = self.server.stopped();
        let mut turns = self.turns.lock().unwrap().take();
        loop {
            // 메일함을 보기 전에 만든다. 그래야 그 사이에 온 메시지를 놓치지 않는다.
            let changed = mailbox.changed();
            match next(&mailbox) {
                Ok(Some(batch)) => {
                    if batch.interrupt {
                        self.stop_shells().await;
                    }
                    match deliver(&api, &batch, PROMPT_RETRY_DELAY).await {
                        Ok(()) => {
                            if let Err(e) = mailbox.ack(&batch.taken) {
                                eprintln!("loom: OpenCode 2 노드의 받음 확인을 쓰지 못했다: {e}");
                            }
                        }
                        // 확인하지 않았으므로 메일함이 나중에 다시 꺼낸다.
                        Err(e) => eprintln!(
                            "loom: OpenCode 2 세션 {}에 메시지를 넘기지 못했다: {e}",
                            self.session
                        ),
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => eprintln!("loom: OpenCode 2 노드의 메일함을 읽지 못했다: {e}"),
            }
            let turn = tokio::select! {
                _ = changed => continue,
                _ = stopped.wait_for(|s| *s) => return,
                Some(turn) = async { turns.as_mut()?.recv().await } => turn,
            };
            // 다시 일하게 하는 글은 턴 뒤에, 열린 질문은 사람이 연 턴 안에 넣는다.
            let (text, inbox) = match turn {
                Turn::Ended(reply) => (mailbox.turn_ended(reply), Inbox::Queue),
                Turn::HumanInput => (mailbox.human_input(), Inbox::Steer),
            };
            if let Some(text) = text
                && let Err(e) = put(&api, &text, inbox, PROMPT_RETRY_DELAY).await
            {
                eprintln!(
                    "loom: OpenCode 2 세션 {}을 다시 일하게 하지 못했다: {e}",
                    self.session
                );
            }
        }
    }
}

/// 세션 입력함의 전달 방식. 서버가 정한 이름이다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Inbox {
    /// 다음 단계 사이(도구 호출 뒤)에 넣는다. 쉬는 중이면 새로 시작한다.
    Steer,
    /// 지금 실행이 끝난 뒤 넣는다. 쉬는 중이면 새로 시작한다.
    Queue,
}

impl Inbox {
    fn as_str(self) -> &'static str {
        match self {
            Inbox::Steer => "steer",
            Inbox::Queue => "queue",
        }
    }
}

/// 메일함에서 한 번에 꺼낸 메시지와 넣을 방식.
#[derive(Debug, PartialEq)]
struct Batch {
    taken: Vec<Delivery>,
    inbox: Inbox,
    interrupt: bool,
}

/// 조정·중단을 먼저 꺼낸다. 없으면 큐를 꺼낸다. 서버 입력함이 상태(일하는 중·쉬는 중)를 스스로 가린다.
fn next(mailbox: &impl Mailbox) -> Result<Option<Batch>, String> {
    let urgent = mailbox.take(&[Kind::Steer, Kind::Interrupt], super::BATCH_LIMIT_BYTES)?;
    if !urgent.is_empty() {
        let interrupt = urgent.iter().any(|d| d.kind == Kind::Interrupt);
        return Ok(Some(Batch {
            taken: urgent,
            inbox: Inbox::Steer,
            interrupt,
        }));
    }
    let queued = mailbox.take(&[Kind::Queue], super::BATCH_LIMIT_BYTES)?;
    Ok((!queued.is_empty()).then_some(Batch {
        taken: queued,
        inbox: Inbox::Queue,
        interrupt: false,
    }))
}

/// 세션 하나에 보내는 요청. 테스트는 가짜 서버로 바꾼다.
trait Api {
    /// 실행을 멈춘다. 셸은 데몬이 이미 끝냈다(`Node::stop_shells`).
    async fn interrupt(&self) -> Result<(), String>;
    /// `id`는 메시지 id다. 같은 id로 다시 보내도 서버는 한 번만 넣는다.
    async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String>;
}

struct Session<'a> {
    server: &'a Server,
    id: &'a str,
    sent: &'a Mutex<HashSet<String>>,
}

impl Api for Session<'_> {
    async fn interrupt(&self) -> Result<(), String> {
        self.server.interrupt(self.id).await
    }

    async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String> {
        // 이벤트가 넣기 응답보다 먼저 올 수 있다. 그래서 보내기 전에 적는다.
        self.sent.lock().unwrap().insert(id.to_owned());
        self.server.prompt(self.id, id, text, inbox.as_str()).await
    }
}

/// 꺼낸 메시지를 세션에 넣는다. 중단이면 실행을 멈춘 뒤 조정으로 넣는다. 멈춘 실행에 남은 큐는 그 뒤에 이어진다(실측).
/// 넣기가 실패하면 같은 id로 `PROMPT_ATTEMPTS`번까지 다시 넣는다. 그래도 실패하면 호출자가 받음을 확인하지 않는다.
async fn deliver(api: &impl Api, batch: &Batch, retry_delay: Duration) -> Result<(), String> {
    if batch.interrupt
        && let Err(e) = api.interrupt().await
    {
        // 멈추지 못해도 본문은 조정으로 들어간다.
        eprintln!("loom: OpenCode 2 실행을 멈추지 못했다: {e}");
    }
    put(
        api,
        &super::message_text(&batch.taken),
        batch.inbox,
        retry_delay,
    )
    .await
}

/// 글 하나를 새 메시지 id로 세션 입력함에 넣는다. 실패하면 같은 id로 `PROMPT_ATTEMPTS`번까지 다시 넣는다.
async fn put(
    api: &impl Api,
    text: &str,
    inbox: Inbox,
    retry_delay: Duration,
) -> Result<(), String> {
    let id = message_id();
    let mut attempt = 1;
    loop {
        match api.prompt(&id, text, inbox).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt == PROMPT_ATTEMPTS => return Err(e),
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(retry_delay).await;
            }
        }
    }
}

/// OpenCode 메시지 id와 같은 모양(`msg_` + 시간 12자리 16진수 + 무작위 14자). 서버는 `msg_` 앞머리만 검사한다.
/// 시간 부분(밀리초 × 4096)은 앞 id보다 늘 크다. 그래서 OpenCode가 만드는 id와 같은 순서로 정렬된다.
fn message_id() -> String {
    static LAST: AtomicU64 = AtomicU64::new(0);
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let now = ms * 0x1000;
    let next = |last: u64| now.max(last + 1);
    let last = LAST
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(next(last))
        })
        .expect("늘 Some을 돌려준다");
    let time = next(last) & 0xFFFF_FFFF_FFFF;
    let mut random = [0u8; 14];
    // 무작위 값을 얻지 못하면 0이 남는다. 시간과 카운터가 이미 id를 가른다.
    let _ = getrandom::fill(&mut random);
    let tail: String = random
        .iter()
        .map(|b| BASE62[*b as usize % BASE62.len()] as char)
        .collect();
    format!("msg_{time:012x}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_text_has_the_ask_rules() {
        super::super::assert_every_launch_asks(|launch| (usage(launch), usage(launch)));
    }
    use serde_json::json;
    use std::sync::Mutex;

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(1),
            from: "부모".into(),
            kind,
            body: body.into(),
            what: Some(crate::protocol::MessageKind::Note),
        }
    }

    /// 꺼낼 때 받은 방식과 상한을 적는 메일함. `kinds`에 맞는 메시지를 온 순서로 모두 꺼낸다.
    #[derive(Default)]
    struct FakeMailbox {
        queued: Mutex<Vec<Delivery>>,
        takes: Mutex<Vec<(Vec<Kind>, usize)>>,
    }

    impl Mailbox for FakeMailbox {
        fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::pending())
        }

        fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String> {
            self.takes
                .lock()
                .unwrap()
                .push((kinds.to_vec(), limit_bytes));
            let mut queued = self.queued.lock().unwrap();
            let (taken, kept) = queued.drain(..).partition(|d| kinds.contains(&d.kind));
            *queued = kept;
            Ok(taken)
        }

        fn ack(&self, _taken: &[Delivery]) -> Result<(), String> {
            Ok(())
        }

        fn turn_ended(&self, _reply: Option<String>) -> Option<String> {
            None
        }

        fn human_input(&self) -> Option<String> {
            None
        }
    }

    /// 2.0.32 이벤트(실측한 모양에서 필요한 필드만 남겼다). 다른 세션의 이벤트는 보지 않는다.
    #[test]
    fn turn_events_come_from_this_session_only() {
        let event = |kind: &str, data: Value| json!({ "type": kind, "data": data });
        let ours = |extra: Value| {
            let mut data = json!({ "sessionID": "s" });
            data.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            data
        };
        assert_eq!(
            turn_event(&event("session.execution.started", ours(json!({}))), "s"),
            Some(TurnEvent::Started)
        );
        assert_eq!(
            turn_event(
                &event(
                    "session.text.ended",
                    ours(json!({ "text": "Shall I go?", "ordinal": 0 }))
                ),
                "s"
            ),
            Some(TurnEvent::Said("Shall I go?".into()))
        );
        assert_eq!(
            turn_event(&event("session.execution.succeeded", ours(json!({}))), "s"),
            Some(TurnEvent::Succeeded)
        );
        for stopped in ["session.execution.failed", "session.execution.interrupted"] {
            assert_eq!(
                turn_event(&event(stopped, ours(json!({}))), "s"),
                Some(TurnEvent::Stopped)
            );
        }
        assert_eq!(
            turn_event(
                &event("session.execution.succeeded", ours(json!({}))),
                "other"
            ),
            None
        );
        assert_eq!(
            turn_event(
                &event("session.text.delta", ours(json!({ "delta": "x" }))),
                "s"
            ),
            None
        );
        let enqueued = |kind: &str| {
            ours(
                json!({ "inboxID": "msg_1", "item": { "type": kind, "payload": { "text": "hi" } } }),
            )
        };
        assert_eq!(
            turn_event(&event("session.inbox.enqueued", enqueued("user")), "s"),
            Some(TurnEvent::Enqueued { id: "msg_1".into() })
        );
        assert_eq!(
            turn_event(&event("session.inbox.enqueued", enqueued("other")), "s"),
            None
        );
    }

    /// 보낸 요청을 적는 가짜 서버. 처음 `fail`번의 넣기는 실패한다.
    #[derive(Default)]
    struct FakeApi {
        fail: u32,
        calls: Mutex<Vec<String>>,
    }

    impl Api for FakeApi {
        async fn interrupt(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("interrupt".into());
            Ok(())
        }

        async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String> {
            let mut calls = self.calls.lock().unwrap();
            calls.push(format!("{} {id} {text}", inbox.as_str()));
            if calls.len() as u32 <= self.fail {
                return Err("연결이 끊겼다".into());
            }
            Ok(())
        }
    }

    #[test]
    fn steer_and_interrupt_go_before_the_queue_and_share_the_batch_limit() {
        let mailbox = FakeMailbox::default();
        *mailbox.queued.lock().unwrap() = vec![
            delivery(Kind::Queue, "a"),
            delivery(Kind::Steer, "b"),
            delivery(Kind::Interrupt, "c"),
        ];
        let first = next(&mailbox).unwrap().unwrap();
        assert_eq!(first.inbox, Inbox::Steer);
        assert!(first.interrupt);
        assert_eq!(
            first.taken,
            [delivery(Kind::Steer, "b"), delivery(Kind::Interrupt, "c")]
        );
        let second = next(&mailbox).unwrap().unwrap();
        assert_eq!(second.inbox, Inbox::Queue);
        assert!(!second.interrupt);
        assert_eq!(second.taken, [delivery(Kind::Queue, "a")]);
        assert_eq!(next(&mailbox).unwrap(), None);
        let limit = crate::profile::BATCH_LIMIT_BYTES;
        assert!(
            mailbox
                .takes
                .lock()
                .unwrap()
                .iter()
                .all(|(_, l)| *l == limit)
        );
    }

    #[tokio::test]
    async fn interrupt_stops_the_run_then_steers_the_body() {
        let api = FakeApi::default();
        let batch = Batch {
            taken: vec![delivery(Kind::Interrupt, "멈춰라")],
            inbox: Inbox::Steer,
            interrupt: true,
        };
        deliver(&api, &batch, Duration::ZERO).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls[0], "interrupt");
        assert!(calls[1].starts_with("steer msg_"), "{}", calls[1]);
        assert!(calls[1].ends_with(
            "loom message #1 (from: 부모, kind: note, mode: interrupt. Stop your current work and do what this message says)\n멈춰라"
        ));
        assert_eq!(calls.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_prompt_retries_with_the_same_id() {
        let api = FakeApi {
            fail: 2,
            ..FakeApi::default()
        };
        let batch = Batch {
            taken: vec![delivery(Kind::Queue, "a")],
            inbox: Inbox::Queue,
            interrupt: false,
        };
        deliver(&api, &batch, PROMPT_RETRY_DELAY).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| *c == calls[0]), "{calls:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn prompt_gives_up_after_bounded_attempts() {
        let api = FakeApi {
            fail: u32::MAX,
            ..FakeApi::default()
        };
        let batch = Batch {
            taken: vec![delivery(Kind::Queue, "a")],
            inbox: Inbox::Queue,
            interrupt: false,
        };
        let err = deliver(&api, &batch, PROMPT_RETRY_DELAY).await.unwrap_err();
        assert_eq!(err, "연결이 끊겼다");
        assert_eq!(api.calls.lock().unwrap().len(), PROMPT_ATTEMPTS as usize);
    }

    #[test]
    fn message_ids_match_the_server_pattern_and_sort_by_time() {
        let a = message_id();
        let b = message_id();
        assert!(a.starts_with("msg_") && a.len() == 4 + 12 + 14, "{a}");
        assert!(a[4..16] < b[4..16], "{a} {b}");
    }

    /// 2.0.32 `GET /api/event`에서 옮긴 이벤트다(위치·durable 필드는 줄였다). TUI는 고른 값을 다음 입력 때 알린다.
    #[test]
    fn model_selection_of_this_session_gives_provider_model_and_variant() {
        let selected = json!({ "type": "session.model.selected", "data": {
            "sessionID": "ses_a", "model": { "id": "kta-chat-deep", "providerID": "KTA", "variant": "high" } } });
        assert_eq!(
            selected_model(&selected, "ses_a"),
            Some(Model {
                model: Some("KTA/kta-chat-deep".into()),
                effort: Some("high".into())
            })
        );
        // variant를 끄면 이벤트에 variant가 없다.
        let default = json!({ "type": "session.model.selected", "data": {
            "sessionID": "ses_a", "model": { "id": "kta-chat-deep", "providerID": "KTA" },
            "previous": { "id": "kta-chat-deep", "providerID": "KTA", "variant": "high" } } });
        assert_eq!(selected_model(&default, "ses_a").unwrap().effort, None);
        assert_eq!(selected_model(&selected, "ses_b"), None);
        let step = json!({ "type": "session.step.started", "data": { "sessionID": "ses_a",
            "model": { "id": "kta-chat-deep", "providerID": "KTA" } } });
        assert_eq!(selected_model(&step, "ses_a"), None);
    }

    #[test]
    fn version_warning_ignores_the_leading_v() {
        assert_eq!(version_warning("opencode v2.0.32\n"), None);
        assert_eq!(
            version_warning("opencode v2.0.33\n"),
            Some(VersionMismatch {
                installed: "2.0.33".into(),
                pinned: "2.0.32".into()
            })
        );
    }

    #[test]
    fn args_attach_the_tui_to_the_node_session() {
        assert_eq!(
            args("http://127.0.0.1:1", "ses_1"),
            ["--server", "http://127.0.0.1:1", "--session", "ses_1"]
        );
    }
}
