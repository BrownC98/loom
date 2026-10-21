//! 상호작용 레이어: 노드 안의 에이전트가 부르는 명령. 데몬에 HTTP로 요청만 보낸다.
//! 데몬 주소와 자기 토큰은 데몬이 노드를 띄울 때 넣은 `LOOM_URL`(`spawn`은 `LOOM_SPAWN_URL`), `LOOM_TOKEN`에서 읽는다.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::daemon::{
    AckRequest, AdoptRequest, AskRequest, DoneRequest, GuardRequest, HookWaitRequest, IdleRequest,
    RemoveRequest, ReplyRequest, SendRequest, SpawnRequest, SpawnRun, TakeRequest, Taken,
    WaitRequest,
};
use crate::graph::{Delivery, Kind, Listed, MessageId, NodeId, Relations, Sent};
use crate::profile::claude;
use crate::profile::{AgentId, ModelChoice};
use crate::profile::{BETWEEN_TOOLS_KINDS, Take, cursor};

/// 서버가 대기를 끝낸 뒤 오류 응답을 보낼 여유 시간이다.
const WAIT_RESPONSE_GRACE_SECS: u64 = 5;
/// 기다리는 명령(`loom wait`, 쉬는 노드의 훅)이 데몬에 닿지 않을 때 다시 붙는 간격과 상한.
/// 데몬은 `scripts/host.sh`·앱 업데이트로 다시 시작해도 노드는 산다. 그동안 끊긴 대기를 다시 연다.
/// 2초: 다시 시작은 보통 몇 초 걸린다. 그 사이 요청을 몇 번만 보낸다.
/// 상한이 지나면 끝낸다. Claude가 죽고 훅만 남았을 때 데몬도 없으면 끝없이 돌지 않게 한다.
const RECONNECT_AFTER: Duration = Duration::from_secs(2);
const RECONNECT_FOR: Duration = Duration::from_secs(10 * 60);
/// Claude `asyncRewake` 훅이 이 코드로 끝나면 stderr를 모델에 넘기고 깨운다.
const REWAKE_EXIT: i32 = 2;

/// `agent`가 없으면 `command`를 셸 명령 한 줄로 띄운다.
pub fn spawn(
    name: String,
    worktree: Option<String>,
    agent: Option<String>,
    file: Option<PathBuf>,
    choice: ModelChoice,
    command: Vec<String>,
) -> Result<(), String> {
    let brief = match file {
        Some(path) => {
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None if std::io::stdin().is_terminal() => String::new(),
        None => {
            let mut brief = String::new();
            std::io::stdin()
                .read_to_string(&mut brief)
                .map_err(|e| e.to_string())?;
            brief
        }
    };
    let run = match agent {
        Some(id) => SpawnRun::Agent(AgentId(id)),
        None => SpawnRun::Command(command.join(" ")),
    };
    let request = SpawnRequest {
        name,
        run,
        brief,
        worktree,
        choice,
    };
    // 프록시를 거치지 않는다. Codex 샌드박스 안에서는 실패하고, 사람이 승인하면 샌드박스 밖에서 닿는다.
    let _: NodeId = post(&env_var("LOOM_SPAWN_URL")?, None, "spawn", &request, None)?;
    println!("Created node: {}", request.name);
    Ok(())
}

pub fn send(to: String, kind: Kind, body: String) -> Result<(), String> {
    let sent: Sent = call("send", &SendRequest { to, kind, body }, None)?;
    print_sent(&sent);
    Ok(())
}

pub fn reply(message: u32, body: String) -> Result<(), String> {
    let sent: Sent = call(
        "reply",
        &ReplyRequest {
            message: MessageId(message),
            body,
        },
        None,
    )?;
    print_sent(&sent);
    Ok(())
}

fn print_sent(sent: &Sent) {
    match sent.recipient {
        Some(to) => println!("loom message #{} → node #{}", sent.id.0, to.0),
        None => println!(
            "Kept loom message #{}. You have no parent now; your next parent gets it.",
            sent.id.0
        ),
    }
}

pub fn status() -> Result<(), String> {
    let relations: Relations = call("status", &(), None)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&relations).map_err(|e| e.to_string())?
    );
    Ok(())
}

pub fn directory() -> Result<(), String> {
    let listed: Vec<Listed> = call("nodes", &(), None)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&listed).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// 출력을 다 쓴 뒤 받음을 확인한다. 확인 전에 죽으면 같은 메시지를 다시 받는다.
/// 데몬이 다시 시작하는 동안에는 다시 붙어 기다린다. 끊긴 대기가 꺼낸 메시지는 데몬이 `REDELIVER_AFTER` 뒤에 다시 준다.
pub fn wait(reply_to: Option<u32>, timeout_secs: Option<u64>, json: bool) -> Result<(), String> {
    let deadline = timeout_secs.map(|secs| Instant::now() + Duration::from_secs(secs));
    let delivery: Delivery = reconnecting(deadline, |remaining| {
        let timeout_secs = remaining.map(|left| left.as_secs());
        let request = WaitRequest {
            reply_to: reply_to.map(MessageId),
            timeout_secs,
        };
        let timeout = timeout_secs
            .map(|secs| Duration::from_secs(secs.saturating_add(WAIT_RESPONSE_GRACE_SECS)));
        call("wait", &request, timeout)
    })?;
    let id = delivery.id;
    if json {
        println!(
            "{}",
            serde_json::to_string(&delivery).map_err(|e| e.to_string())?
        );
    } else {
        println!("{}", crate::profile::message_text(&[delivery]));
    }
    // 파이프에 본문을 넘기기 전에 확인하지 않는다.
    std::io::Write::flush(&mut std::io::stdout()).map_err(|e| e.to_string())?;
    reconnecting(None, |_| {
        call::<()>("ack", &AckRequest { ids: vec![id] }, None)
    })
    .map_err(|e| {
        format!(
            "Could not acknowledge: {e}. Message #{} can come again.",
            id.0
        )
    })
}

/// Claude 훅: stdin의 훅 입력을 읽고, 이 지점에서 넘길 메시지를 데몬에서 꺼내 훅 출력으로 쓴다.
/// 모델이 바뀌었는지는 데몬이 본다. 알리지 못해도 메시지 전달은 계속한다.
pub fn hook() -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("hook input: {e}"))?;
    let event = claude::Event::deserialize(&input).map_err(|e| format!("hook input: {e}"))?;
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    if let Some(model) = claude::reported_model(&event, &input)
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    let request = HookWaitRequest {
        wait: event.human_wait(),
    };
    if let Err(e) = call::<()>("hook-wait", &request, Some(timeout)) {
        eprintln!("loom: could not report the human wait: {e}");
    }
    let transcript = input["transcript_path"].as_str().map(str::to_owned);
    let (kinds, notice_queue) = match event.take() {
        Take::Now(kinds) => (kinds, false),
        Take::BetweenTools => (BETWEEN_TOOLS_KINDS, true),
        Take::Never => return Ok(()),
        Take::WhenIdle => {
            let taken = wait_for_mail(&IdleRequest {
                transcript,
                rate_limited: event.rate_limited(),
            })?;
            if taken.is_empty() {
                return Ok(());
            }
            let mut stderr = std::io::stderr();
            writeln!(stderr, "{}", crate::profile::message_text(&taken))
                .map_err(|e| e.to_string())?;
            stderr.flush().map_err(|e| e.to_string())?;
            acknowledge(&taken, None);
            std::process::exit(REWAKE_EXIT)
        }
    };
    let request = TakeRequest {
        kinds: kinds.to_vec(),
        limit_bytes: claude::HOOK_LIMIT_BYTES,
        transcript,
        notice_queue,
    };
    let taken: Taken = call("take", &request, Some(timeout))?;
    if let Some(reply) = event.reply(&taken.messages, &taken.queued_from) {
        println!("{reply}");
    }
    // 확인하지 못해도 훅 출력은 Claude에 넘긴다. 데몬이 같은 메시지를 다시 주고, 본문의 `#id`로 같은 메시지임을 안다.
    acknowledge(&taken.messages, Some(timeout));
    Ok(())
}

/// Cursor 훅: Claude 훅과 같은 일을 Cursor 입출력 형식으로 한다. 출력이 늘 JSON이어야 Cursor가 훅을 실패로 보지 않는다.
/// 쉬는 노드의 stop 훅은 메시지를 새 턴 입력으로 넘긴다. 사람이 그 사이에 입력하면 데몬이 대기를 끝낸다.
pub fn cursor_hook() -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("hook input: {e}"))?;
    let event = cursor::Event::deserialize(&input).map_err(|e| format!("hook input: {e}"))?;
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    if let Some(model) = event.model()
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    let take_now = |kinds: &[Kind], notice_queue| {
        call(
            "take",
            &TakeRequest {
                kinds: kinds.to_vec(),
                limit_bytes: cursor::HOOK_LIMIT_BYTES,
                transcript: None,
                notice_queue,
            },
            Some(timeout),
        )
    };
    let taken = match event.take() {
        None | Some(Take::Never) => Taken {
            messages: Vec::new(),
            queued_from: Vec::new(),
        },
        Some(Take::WhenIdle) => Taken {
            messages: wait_for_mail(&IdleRequest::default())?,
            queued_from: Vec::new(),
        },
        Some(Take::Now(kinds)) => take_now(kinds, false)?,
        Some(Take::BetweenTools) => take_now(BETWEEN_TOOLS_KINDS, true)?,
    };
    println!("{}", event.reply(&taken.messages, &taken.queued_from));
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    acknowledge(&taken.messages, Some(timeout));
    Ok(())
}

/// Cursor의 둘째 sessionStart 훅: 상시 지시를 넘긴다. 첫 훅과 나누는 이유는 `cursor::CONTEXT_LIMIT_CHARS`다.
pub fn cursor_rules() -> Result<(), String> {
    let position = cursor::position_from(std::env::var(cursor::POSITION_ENV).ok().as_deref());
    println!("{}", cursor::rules(position));
    Ok(())
}

/// 꺼낸 메시지를 받는 쪽에 넘겼다고 데몬에 알린다. 실패해도 훅은 끝낸다. 데몬이 다시 준다.
fn acknowledge(taken: &[Delivery], timeout: Option<Duration>) {
    if taken.is_empty() {
        return;
    }
    let ids = taken.iter().map(|d| d.id).collect();
    if let Err(e) = call::<()>("ack", &AckRequest { ids }, timeout) {
        eprintln!("loom: could not acknowledge: {e}");
    }
}

/// 쉬는 노드의 훅이 메시지를 기다린다. Claude 훅은 본문을 stderr에 쓰고 `REWAKE_EXIT`로, Cursor 훅은 새 턴 입력으로 넘긴다.
/// 데몬이 대기를 끝내면(새 대기, 새 턴) 빈 목록이다.
fn wait_for_mail(request: &IdleRequest) -> Result<Vec<Delivery>, String> {
    reconnecting(None, |_| call("idle", request, None))
}

/// 데몬에 닿지 않으면 `RECONNECT_AFTER`마다 다시 부른다. 데몬의 거절은 바로 돌려준다.
/// `attempt`는 `deadline`까지 남은 시간을 받는다. 남은 시간이 다시 붙기 전에 끝나면 닿지 않은 오류로 끝낸다.
fn reconnecting<T>(
    deadline: Option<Instant>,
    mut attempt: impl FnMut(Option<Duration>) -> Result<T, CallError>,
) -> Result<T, String> {
    let mut unreachable_since = None;
    loop {
        let started = Instant::now();
        match attempt(deadline.map(|d| d.saturating_duration_since(started))) {
            Ok(value) => return Ok(value),
            Err(CallError::Unreachable(e)) => {
                // 오래 붙어 있다 끊겼으면 데몬이 방금 다시 시작했다. 상한을 처음부터 잰다.
                if started.elapsed() >= RECONNECT_AFTER {
                    unreachable_since = None;
                }
                // 끊긴 때부터 잰다. 긴 대기를 시작한 때부터 재면 10분 넘게 기다린 대기는 끊기자마자 포기한다(2026-10-21 배포 때 총괄의 `loom wait`).
                let gave_up =
                    unreachable_since.get_or_insert_with(Instant::now).elapsed() >= RECONNECT_FOR;
                if gave_up || deadline.is_some_and(|d| Instant::now() + RECONNECT_AFTER >= d) {
                    return Err(e);
                }
                std::thread::sleep(RECONNECT_AFTER);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// 이 명령을 부른 셸을 중단 대상으로 데몬에 알린다. Claude 훅이 Bash 명령 앞에 붙인다.
/// 셸은 데몬이 이 프로세스의 조상에서 고른다. 이 프로세스는 응답을 받을 때까지 살아 있다.
pub fn guard() -> Result<(), String> {
    let caller = std::process::id();
    Ok(call::<()>("guard", &GuardRequest { caller }, None)?)
}

/// 사람에게 묻는다: 받은편지함에 질문 한 줄을 띄우고 바로 끝난다. 답은 사람이 노드 터미널에 쓴다.
pub fn ask(question: String) -> Result<(), String> {
    call::<()>("ask", &AskRequest { question }, None)?;
    println!(
        "The question is in the human's inbox. End your turn now. The human answers in your terminal."
    );
    Ok(())
}

pub fn done(summary: Option<String>, file: Option<PathBuf>) -> Result<(), String> {
    let summary = match file {
        // 부모가 다른 폴더에서 읽어도 찾도록 절대 경로로 보낸다. 파일이 없으면 여기서 실패한다.
        Some(path) => std::fs::canonicalize(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .display()
            .to_string(),
        None => summary.unwrap_or_default(),
    };
    let alive_children: Vec<String> = call("done", &DoneRequest { summary }, None)?;
    println!(
        "Reported to your parent. If you have no parent, loom keeps the report for your next parent."
    );
    if !alive_children.is_empty() {
        println!(
            "Warning: these children are still running: {}. Reclaim each with `loom rm <name>` when you keep its results, or they stay in the graph.",
            alive_children.join(", ")
        );
    }
    Ok(())
}

pub fn adopt(name: String) -> Result<(), String> {
    call::<()>("adopt", &AdoptRequest { name }, None)?;
    println!("You are the parent now. Messages that node kept move to your inbox.");
    Ok(())
}

pub fn remove(name: String) -> Result<(), String> {
    let moved: Vec<String> = call("rm", &RemoveRequest { name: name.clone() }, None)?;
    println!("Removed {name}.");
    if !moved.is_empty() {
        println!(
            "Its children are now your children: {}. Reclaim them next with `loom rm <name>` when you keep their results.",
            moved.join(", ")
        );
    }
    Ok(())
}

/// 노드 문으로 보낸다. Codex 샌드박스 안에서는 Codex가 넣은 프록시 환경 변수로만 닿는다.
/// `timeout`이 없으면 기다림 상한을 두지 않는다. `wait`는 메시지가 올 때까지 끝나지 않는다.
fn call<T: DeserializeOwned>(
    action: &str,
    body: &impl Serialize,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let url = env_var("LOOM_URL").map_err(CallError::Refused)?;
    post(&url, ureq::Proxy::try_from_env(), action, body, timeout)
}

/// 데몬 요청의 실패. 기다리는 명령은 닿지 않을 때만 다시 붙는다(`reconnecting`).
enum CallError {
    /// 데몬에 닿지 않았거나, 데몬이 지금 답하지 못했거나(5xx), 응답을 다 받기 전에 연결이 끊겼다. 다시 부르면 된다.
    Unreachable(String),
    /// 데몬이 거절했거나(4xx) 응답 형식을 읽지 못했다. 다시 불러도 같다.
    Refused(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Unreachable(e) | CallError::Refused(e) => f.write_str(e),
        }
    }
}

impl From<CallError> for String {
    fn from(e: CallError) -> Self {
        e.to_string()
    }
}

fn env_var(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set. Run this only inside a loom node."))
}

fn post<T: DeserializeOwned>(
    url: &str,
    proxy: Option<ureq::Proxy>,
    action: &str,
    body: &impl Serialize,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let token = env_var("LOOM_TOKEN").map_err(CallError::Refused)?;
    post_as(url, &token, proxy, action, body, timeout)
}

fn post_as<T: DeserializeOwned>(
    url: &str,
    token: &str,
    proxy: Option<ureq::Proxy>,
    action: &str,
    body: &impl Serialize,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    // 거부 문장을 본문에서 읽으려고 4xx·5xx를 오류로 바꾸지 않는다.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(timeout)
        .proxy(proxy)
        .build()
        .into();
    let mut res = agent
        .post(format!("{url}/api/agent/{action}"))
        .header("authorization", format!("Bearer {token}"))
        .send_json(body)
        // Codex 모델은 사용법 지시보다 이 출력을 보고 승인을 요청한다(실측).
        .map_err(|e| {
            CallError::Unreachable(format!("Could not connect to the daemon: {e}. In a sandbox, get approval to run outside the sandbox and try again."))
        })?;
    let status = res.status();
    // 데몬이 응답 도중에 끝나면(다시 시작) 본문이 끊긴다. 쉬는 노드의 훅이 이 오류로 끝나면 그 노드는 다시 깨지 않는다.
    let text = res.body_mut().read_to_string().map_err(|e| {
        CallError::Unreachable(format!(
            "The daemon closed the connection before its answer was complete: {e}."
        ))
    })?;
    // 5xx: 데몬이 지금 답하지 못했다. 데몬 앞의 프록시(Codex 샌드박스)는 데몬이 다시 시작하는 동안 502~504로 답한다.
    if status.is_server_error() {
        return Err(CallError::Unreachable(format!(
            "The daemon could not answer ({status}): {text}"
        )));
    }
    if !status.is_success() {
        return Err(CallError::Refused(text));
    }
    serde_json::from_str(&text).map_err(|e| CallError::Refused(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreachable() -> CallError {
        CallError::Unreachable("down".into())
    }

    #[test]
    fn reconnects_while_the_daemon_restarts_and_keeps_the_deadline() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut left = Vec::new();
        let got = reconnecting(Some(deadline), |remaining| {
            left.push(remaining.unwrap());
            if left.len() < 2 {
                Err(unreachable())
            } else {
                Ok("mail")
            }
        });
        assert_eq!(got, Ok("mail"));
        // 다시 붙을 때는 남은 시간만 기다린다.
        assert!(left[1] <= left[0] - RECONNECT_AFTER);
    }

    #[test]
    fn stops_at_the_deadline_and_on_refusal() {
        let started = Instant::now();
        let mut calls = 0;
        let got: Result<(), String> = reconnecting(Some(started + Duration::from_secs(1)), |_| {
            calls += 1;
            Err(unreachable())
        });
        assert_eq!(got, Err("down".into()));
        assert_eq!(calls, 1);
        assert!(started.elapsed() < RECONNECT_AFTER);
        let got: Result<(), String> =
            reconnecting(None, |_| Err(CallError::Refused("gone".into())));
        assert_eq!(got, Err("gone".into()));
    }

    /// 요청 하나를 다 읽고 `answer`를 그대로 쓴 뒤 연결을 끊는 서버의 주소.
    fn answering(answer: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            // 몸통은 `null`이다. 다 읽기 전에 끊으면 클라이언트가 연결 재설정을 받는다.
            while !request.ends_with(b"null") {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
            }
            stream.write_all(answer.as_bytes()).unwrap();
        });
        url
    }

    fn ask(url: &str) -> Result<Vec<u32>, CallError> {
        post_as(url, "token", None, "idle", &(), None)
    }

    /// 쉬는 노드의 훅은 데몬이 답하지 못한 동안(5xx, 끊긴 응답)에도 계속 기다린다. 2026-10-21 #169가 85분 깨지 않았다.
    #[test]
    fn a_failed_or_cut_answer_is_retried_but_a_refusal_is_not() {
        let failed = "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 6\r\n\r\nlocked";
        let cut = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 50\r\n\r\n[{\"id\"";
        let mail =
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 3\r\n\r\n[7]";
        let urls = [answering(failed), answering(cut), answering(mail)];
        let mut next = urls.iter();
        let got = reconnecting(None, |_| ask(next.next().unwrap()));
        assert_eq!(got, Ok(vec![7]));
        let gone = "HTTP/1.1 410 Gone\r\ncontent-length: 4\r\n\r\ngone";
        assert!(matches!(ask(&answering(gone)), Err(CallError::Refused(e)) if e == "gone"));
    }
}
