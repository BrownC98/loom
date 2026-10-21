//! 상호작용 레이어: 노드 안의 에이전트가 부르는 명령. 데몬에 HTTP로 요청만 보낸다.
//! 데몬 주소와 자기 토큰은 데몬이 노드를 띄울 때 넣은 `LOOM_URL`(`spawn`은 `LOOM_SPAWN_URL`), `LOOM_TOKEN`에서 읽는다.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::daemon::{
    AckRequest, AdoptRequest, DoneRequest, GuardRequest, IdleRequest, RemoveRequest, ReplyRequest,
    SendRequest, SpawnRequest, SpawnRun, TakeRequest, WaitRequest,
};
use crate::graph::{Delivery, Kind, Listed, MessageId, NodeId, Relations, Sent};
use crate::profile::claude::{self, Take};
use crate::profile::{AgentId, ModelChoice};

/// 서버가 대기를 끝낸 뒤 오류 응답을 보낼 여유 시간이다.
const WAIT_RESPONSE_GRACE_SECS: u64 = 5;
/// 쉬는 노드의 훅이 데몬에 닿지 않을 때 다시 붙는 간격과 상한.
/// 데몬은 `scripts/host.sh`·앱 업데이트로 다시 시작해도 노드는 산다. 그동안 끊긴 대기를 다시 연다.
/// 상한이 지나면 끝낸다. Claude가 죽고 훅만 남았을 때 데몬도 없으면 끝없이 돌지 않게 한다.
const IDLE_RETRY_AFTER: Duration = Duration::from_secs(2);
const IDLE_RETRY_FOR: Duration = Duration::from_secs(10 * 60);
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
pub fn wait(reply_to: Option<u32>, timeout_secs: Option<u64>, json: bool) -> Result<(), String> {
    let request = WaitRequest {
        reply_to: reply_to.map(MessageId),
        timeout_secs,
    };
    let timeout =
        timeout_secs.map(|secs| Duration::from_secs(secs.saturating_add(WAIT_RESPONSE_GRACE_SECS)));
    let delivery: Delivery = call("wait", &request, timeout)?;
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
    call::<()>("ack", &AckRequest { ids: vec![id] }, None).map_err(|e| {
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
    let transcript = input["transcript_path"].as_str().map(str::to_owned);
    let kinds = match event.take() {
        Take::Now(kinds) => kinds,
        Take::WhenIdle => {
            return wake_on_mail(&IdleRequest {
                transcript,
                rate_limited: event.rate_limited(),
            });
        }
    };
    let request = TakeRequest {
        kinds: kinds.to_vec(),
        limit_bytes: claude::HOOK_LIMIT_BYTES,
        transcript,
    };
    let taken: Vec<Delivery> = call("take", &request, Some(timeout))?;
    if let Some(reply) = event.reply(&taken) {
        println!("{reply}");
    }
    if !taken.is_empty() {
        // 확인하지 못해도 훅 출력은 Claude에 넘긴다. 데몬이 같은 메시지를 다시 주고, 본문의 `#id`로 같은 메시지임을 안다.
        let ids = taken.iter().map(|d| d.id).collect();
        if let Err(e) = call::<()>("ack", &AckRequest { ids }, Some(timeout)) {
            eprintln!("loom: could not acknowledge: {e}");
        }
    }
    Ok(())
}

/// 쉬는 노드의 비동기 훅: 메시지가 오면 본문을 stderr에 쓰고 `REWAKE_EXIT`로 끝난다.
/// 데몬이 대기를 끝내면(새 대기, 새 턴) 아무것도 쓰지 않고 끝난다.
fn wake_on_mail(request: &IdleRequest) -> Result<(), String> {
    let mut unreachable_since = None;
    let taken: Vec<Delivery> = loop {
        let attempt = Instant::now();
        match call("idle", request, None) {
            Ok(taken) => break taken,
            Err(CallError::Unreachable(e)) => {
                // 오래 붙어 있다 끊겼으면 데몬이 방금 다시 시작했다. 상한을 처음부터 잰다.
                if attempt.elapsed() >= IDLE_RETRY_AFTER {
                    unreachable_since = None;
                }
                if unreachable_since.get_or_insert(attempt).elapsed() >= IDLE_RETRY_FOR {
                    return Err(e);
                }
                std::thread::sleep(IDLE_RETRY_AFTER);
            }
            Err(e) => return Err(e.into()),
        }
    };
    if taken.is_empty() {
        return Ok(());
    }
    let mut stderr = std::io::stderr();
    writeln!(stderr, "{}", crate::profile::message_text(&taken)).map_err(|e| e.to_string())?;
    stderr.flush().map_err(|e| e.to_string())?;
    let ids = taken.iter().map(|d| d.id).collect();
    if let Err(e) = call::<()>("ack", &AckRequest { ids }, None) {
        eprintln!("loom: could not acknowledge: {e}");
    }
    std::process::exit(REWAKE_EXIT)
}

/// 이 명령을 부른 셸을 중단 대상으로 데몬에 알린다. Claude 훅이 Bash 명령 앞에 붙인다.
/// 셸은 데몬이 이 프로세스의 조상에서 고른다. 이 프로세스는 응답을 받을 때까지 살아 있다.
pub fn guard() -> Result<(), String> {
    let caller = std::process::id();
    Ok(call::<()>("guard", &GuardRequest { caller }, None)?)
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

/// 데몬 요청의 실패. 쉬는 노드의 훅은 닿지 않을 때만 다시 붙는다.
enum CallError {
    /// 데몬에 닿지 않았거나 응답 전에 연결이 끊겼다.
    Unreachable(String),
    /// 데몬이 거절했거나 응답을 읽지 못했다.
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
    if !res.status().is_success() {
        return Err(CallError::Refused(
            res.body_mut().read_to_string().unwrap_or_default(),
        ));
    }
    res.body_mut()
        .read_json()
        .map_err(|e| CallError::Refused(e.to_string()))
}
