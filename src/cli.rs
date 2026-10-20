//! 상호작용 레이어: 노드 안의 에이전트가 부르는 명령. 데몬에 HTTP로 요청만 보낸다.
//! 데몬 주소와 자기 토큰은 데몬이 노드를 띄울 때 넣은 `LOOM_URL`(`spawn`은 `LOOM_SPAWN_URL`), `LOOM_TOKEN`에서 읽는다.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::daemon::{
    AckRequest, AdoptRequest, DoneRequest, GuardRequest, SendRequest, SpawnRequest, SpawnRun,
    TakeRequest,
};
use crate::graph::{Delivery, Kind, NodeId};
use crate::profile::{AgentId, claude};

/// `agent`가 없으면 `command`를 셸 명령 한 줄로 띄운다.
pub fn spawn(
    name: String,
    agent: Option<String>,
    file: Option<PathBuf>,
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
    let request = SpawnRequest { name, run, brief };
    // 프록시를 거치지 않는다. Codex 샌드박스 안에서는 실패하고, 사람이 승인하면 샌드박스 밖에서 닿는다.
    let _: NodeId = post(&env_var("LOOM_SPAWN_URL")?, None, "spawn", &request, None)?;
    println!("노드를 만들었다: {}", request.name);
    Ok(())
}

pub fn send(to: String, kind: Kind, body: String) -> Result<(), String> {
    let to: Option<NodeId> = call("send", &SendRequest { to, kind, body }, None)?;
    if to.is_none() {
        println!("부모가 없어 보관했다. 새 부모가 생기면 받는다.");
    }
    Ok(())
}

/// 출력을 다 쓴 뒤 받음을 확인한다. 확인 전에 이 프로세스가 죽으면 데몬이 같은 메시지를 다시 준다.
pub fn wait() -> Result<(), String> {
    let Delivery { id, from, body, .. } = call("wait", &(), None)?;
    println!("loom 메시지 #{} (보낸 노드: {from})\n{body}", id.0);
    call::<()>("ack", &AckRequest { ids: vec![id] }, None).map_err(|e| {
        format!(
            "받음 확인을 보내지 못했다: {e}. 같은 메시지 #{}가 다시 올 수 있다.",
            id.0
        )
    })
}

/// Claude 훅: stdin의 훅 입력을 읽고, 이 지점에서 넘길 메시지를 데몬에서 꺼내 훅 출력으로 쓴다.
/// 모델이 바뀌었는지는 데몬이 본다. 알리지 못해도 메시지 전달은 계속한다.
pub fn hook() -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("훅 입력: {e}"))?;
    let event = claude::Event::deserialize(&input).map_err(|e| format!("훅 입력: {e}"))?;
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    if let Some(model) = claude::reported_model(&event, &input)
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: 모델을 알리지 못했다: {e}");
    }
    if event.kinds().is_empty() {
        return Ok(());
    }
    let request = TakeRequest {
        kinds: event.kinds().to_vec(),
        limit_bytes: claude::HOOK_LIMIT_BYTES,
    };
    let taken: Vec<Delivery> = call("take", &request, Some(timeout))?;
    if let Some(reply) = event.reply(&taken) {
        println!("{reply}");
    }
    if taken.is_empty() {
        return Ok(());
    }
    // 확인하지 못해도 훅 출력은 Claude에 넘긴다. 데몬이 같은 메시지를 다시 주고, 본문의 `#id`로 같은 메시지임을 안다.
    let ids = taken.iter().map(|d| d.id).collect();
    if let Err(e) = call::<()>("ack", &AckRequest { ids }, Some(timeout)) {
        eprintln!("loom: 받음 확인을 보내지 못했다: {e}");
    }
    Ok(())
}

/// 이 명령을 부른 셸을 중단 대상으로 데몬에 알린다. Claude 훅이 Bash 명령 앞에 붙인다.
/// 셸은 데몬이 이 프로세스의 조상에서 고른다. 이 프로세스는 응답을 받을 때까지 살아 있다.
pub fn guard() -> Result<(), String> {
    let caller = std::process::id();
    call::<()>("guard", &GuardRequest { caller }, None)
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
    call::<()>("done", &DoneRequest { summary }, None)?;
    println!("부모에게 결과를 보냈다. 부모가 없으면 새 부모가 생길 때 받는다.");
    Ok(())
}

pub fn adopt(name: String) -> Result<(), String> {
    call::<()>("adopt", &AdoptRequest { name }, None)?;
    println!("부모가 되었다. 그 노드가 보관한 메시지가 메일함에 들어온다.");
    Ok(())
}

/// 노드 문으로 보낸다. Codex 샌드박스 안에서는 Codex가 넣은 프록시 환경 변수로만 닿는다.
/// `timeout`이 없으면 기다림 상한을 두지 않는다. `wait`는 메시지가 올 때까지 끝나지 않는다.
fn call<T: DeserializeOwned>(
    action: &str,
    body: &impl Serialize,
    timeout: Option<Duration>,
) -> Result<T, String> {
    let url = env_var("LOOM_URL")?;
    post(&url, ureq::Proxy::try_from_env(), action, body, timeout)
}

fn env_var(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name}가 없다. loom 노드 안에서만 부른다."))
}

fn post<T: DeserializeOwned>(
    url: &str,
    proxy: Option<ureq::Proxy>,
    action: &str,
    body: &impl Serialize,
    timeout: Option<Duration>,
) -> Result<T, String> {
    let token = env_var("LOOM_TOKEN")?;
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
            format!("데몬에 연결하지 못했다: {e}. 샌드박스 안이면 샌드박스 밖 실행 승인을 받아 다시 부른다.")
        })?;
    if !res.status().is_success() {
        return Err(res.body_mut().read_to_string().unwrap_or_default());
    }
    res.body_mut().read_json().map_err(|e| e.to_string())
}
