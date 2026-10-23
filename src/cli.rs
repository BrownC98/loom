//! 상호작용 레이어: 노드 안의 에이전트가 부르는 명령. 데몬에 HTTP로 요청만 보낸다.
//! 데몬 주소와 자기 토큰은 데몬이 노드를 띄울 때 넣은 `LOOM_URL`(`spawn`은 `LOOM_SPAWN_URL`), `LOOM_TOKEN`에서 읽는다.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::daemon::gate::{
    BranchRequest, Deploy, FailRequest, MergeInto, MergePlan, MergeResult, MergedRequest, Readied,
    TOOL_HEADER, VERSION_HEADER, VerdictRequest,
};
use crate::daemon::{
    AckRequest, AdoptRequest, AnsweredRequest, AskRequest, Asked, DoneRequest, GuardRequest,
    HookWaitRequest, IdleRequest, RelaunchRequest, Relaunched, RemoveRequest, ReplyRequest,
    SaidRequest, SendRequest, SpawnRequest, SpawnRun, TakeRequest, Taken, WaitRequest,
};
use crate::graph::{Delivery, Kind, Listed, MessageId, NodeId, Relations, Sent, WaitId};
use crate::profile::claude;
use crate::profile::{AgentId, ModelChoice};
use crate::profile::{Take, cursor};
use crate::protocol::{self, GateState, Judgment};
use crate::roles::Roles;

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

/// `agent`와 `role`이 모두 없으면 `command`를 셸 명령 한 줄로 띄운다.
pub fn spawn(
    name: String,
    worktree: Option<String>,
    agent: Option<String>,
    role: Option<String>,
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
    let run = match (agent, &role) {
        (None, None) => SpawnRun::Command(command.join(" ")),
        (agent, _) => SpawnRun::Agent(agent.map(AgentId)),
    };
    let request = SpawnRequest {
        name,
        run,
        brief,
        worktree,
        choice,
        role,
    };
    // 프록시를 거치지 않는다. Codex 샌드박스 안에서는 실패하고, 사람이 승인하면 샌드박스 밖에서 닿는다.
    let _: NodeId = post(&env_var("LOOM_SPAWN_URL")?, None, "spawn", &request, None)?;
    println!("Created node: {}", request.name);
    Ok(())
}

pub fn roles() -> Result<(), String> {
    let roles: Roles = call("roles", &(), None)?;
    for role in roles.0 {
        let run = match role.agent {
            None => "띄우지 않음".to_owned(),
            Some(agent) => {
                let or_default = |v: Option<String>| v.unwrap_or_else(|| "CLI 기본".into());
                format!(
                    "{}·{}·{}",
                    agent.0,
                    or_default(role.model),
                    or_default(role.effort)
                )
            }
        };
        println!("{} | {run} | {}", role.name, role.duty);
    }
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
    let kinds = match event.take() {
        Take::Now(kinds) => kinds,
        Take::Never => return Ok(()),
        Take::WhenIdle => {
            if let claude::Event::Stop = event
                && let Some(nudge) = turn_end(claude::last_reply(&input), timeout)
            {
                // 메시지는 꺼내지 않는다. 다시 일한 턴이 끝나면 이 훅이 다시 꺼낸다.
                to_model(&nudge)?;
                std::process::exit(REWAKE_EXIT)
            }
            let taken = wait_for_mail(&IdleRequest {
                transcript,
                rate_limited: event.rate_limited(),
            })?;
            if taken.is_empty() {
                return Ok(());
            }
            to_model(&crate::profile::message_text(&taken))?;
            acknowledge(&taken, None);
            std::process::exit(REWAKE_EXIT)
        }
    };
    let request = TakeRequest {
        kinds: kinds.to_vec(),
        limit_bytes: claude::HOOK_LIMIT_BYTES,
        transcript,
        human_input: event.human_input(),
    };
    let taken: Taken = call("take", &request, Some(timeout))?;
    if let Some(reply) = event.reply(&taken.messages, taken.note.as_deref()) {
        println!("{reply}");
    }
    // 확인하지 못해도 훅 출력은 Claude에 넘긴다. 데몬이 같은 메시지를 다시 주고, 본문의 `#id`로 같은 메시지임을 안다.
    acknowledge(&taken.messages, Some(timeout));
    Ok(())
}

/// 비동기 훅의 글은 stderr로 모델에 간다(`REWAKE_EXIT`와 함께).
fn to_model(text: &str) -> Result<(), String> {
    let mut stderr = std::io::stderr();
    writeln!(stderr, "{text}").map_err(|e| e.to_string())?;
    stderr.flush().map_err(|e| e.to_string())
}

/// 턴 끝 질문 검사. `reply`는 이 턴의 답 글이다(Cursor는 `afterAgentResponse` 훅이 먼저 넘겼다).
/// 다시 일하게 할 글이 있으면 돌려준다. 데몬에 닿지 못하면 검사하지 않는다.
fn turn_end(reply: Option<String>, timeout: Duration) -> Option<String> {
    if let Some(text) = reply
        && let Err(e) = call::<()>("said", &SaidRequest { text }, Some(timeout))
    {
        eprintln!("loom: could not report the reply: {e}");
    }
    call::<Option<String>>("turn-end", &(), Some(timeout)).unwrap_or_else(|e| {
        eprintln!("loom: could not check the turn end: {e}");
        None
    })
}

/// Cursor 훅: Claude 훅과 같은 일을 Cursor 입출력 형식으로 한다. 출력이 늘 JSON이어야 Cursor가 훅을 실패로 보지 않는다.
/// 쉬는 노드의 stop 훅은 메시지를 새 턴 입력으로 넘긴다. 사람이 그 사이에 입력하면 데몬이 대기를 끝낸다.
pub fn cursor_hook() -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("hook input: {e}"))?;
    let event = cursor::Event::deserialize(&input).map_err(|e| format!("hook input: {e}"))?;
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    let transcript = input["transcript_path"].as_str().map(str::to_owned);
    if let Some(model) = event.model()
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    let take_now = |kinds: &[Kind]| {
        call(
            "take",
            &TakeRequest {
                kinds: kinds.to_vec(),
                limit_bytes: cursor::HOOK_LIMIT_BYTES,
                transcript: transcript.clone(),
                human_input: matches!(event, cursor::Event::BeforeSubmitPrompt { .. }),
            },
            Some(timeout),
        )
    };
    if let cursor::Event::AfterAgentResponse { text } = &event
        && let Err(e) = call::<()>("said", &SaidRequest { text: text.clone() }, Some(timeout))
    {
        eprintln!("loom: could not report the reply: {e}");
    }
    if let cursor::Event::Stop { .. } = event
        && let Some(nudge) = turn_end(None, timeout)
    {
        // 메시지는 꺼내지 않는다. 다시 일한 턴의 stop 훅이 꺼낸다.
        println!("{}", serde_json::json!({ "followup_message": nudge }));
        return Ok(());
    }
    let taken = match event.take() {
        None | Some(Take::Never) => Taken {
            messages: Vec::new(),
            note: None,
        },
        Some(Take::WhenIdle) => Taken {
            messages: wait_for_mail(&IdleRequest {
                transcript,
                rate_limited: false,
            })?,
            note: None,
        },
        Some(Take::Now(kinds)) => take_now(kinds)?,
    };
    println!("{}", event.reply(&taken.messages, taken.note.as_deref()));
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    acknowledge(&taken.messages, Some(timeout));
    Ok(())
}

/// Cursor의 둘째 sessionStart 훅: 상시 지시를 넘긴다. 첫 훅과 나누는 이유는 `cursor::CONTEXT_LIMIT_CHARS`다.
pub fn cursor_rules() -> Result<(), String> {
    let env = |key| std::env::var(key).ok();
    let launch = crate::skills::Launch {
        position: cursor::position_from(env(cursor::POSITION_ENV).as_deref()),
        skills: cursor::skills_from(env(cursor::SKILLS_ENV).as_deref()),
    };
    println!("{}", cursor::rules(launch));
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
/// `doc`(`--doc`)은 문서 하나를 질문에 붙인다. 사람은 질문 줄의 링크로 연다.
pub fn ask(question: String, doc: Option<PathBuf>) -> Result<(), String> {
    let doc = doc.as_deref().map(absolute).transpose()?;
    let asked: Asked = call(
        "ask",
        &AskRequest {
            question: question.clone(),
            doc,
        },
        None,
    )?;
    let with = asked
        .doc
        .map(|doc| format!(" with {} ({})", doc.id, doc.title))
        .unwrap_or_default();
    println!("{} in the inbox{with}: {}", asked.id, question.trim());
    if asked.open.len() > 1 {
        let open: Vec<String> = asked.open.iter().map(WaitId::to_string).collect();
        println!("Open questions: {}.", open.join(", "));
    }
    println!(
        "The question is in the human's inbox. End your turn now. The human answers in your terminal or in the inbox."
    );
    Ok(())
}

/// `ids`는 `Q12`나 `12`다. 없으면 열린 질문이 하나일 때만 그것을 닫는다.
pub fn answered(ids: Vec<String>) -> Result<(), String> {
    let ids = ids
        .iter()
        .map(|id| {
            id.trim_start_matches(['Q', 'q'])
                .parse()
                .map(WaitId)
                .map_err(|_| format!("{id}: give a question number such as Q12."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let closed: Vec<WaitId> = call("answered", &AnsweredRequest { ids }, None)?;
    let closed: Vec<String> = closed.iter().map(WaitId::to_string).collect();
    match closed.is_empty() {
        true => println!("No question was open."),
        false => println!(
            "Closed {}. They are no longer in the human's inbox.",
            closed.join(", ")
        ),
    }
    Ok(())
}

/// `doc`(`--doc`)은 사람의 받은편지함 완료 줄에 문서 링크를 붙인다. 사람이 띄운 노드만 쓴다.
pub fn done(
    summary: Option<String>,
    file: Option<PathBuf>,
    asked: Option<String>,
    images: Vec<PathBuf>,
    doc: Option<PathBuf>,
) -> Result<(), String> {
    let summary = match file {
        Some(path) => {
            let path = absolute(&path)?;
            // 사람은 완료 줄의 글 속 경로를 열지 못한다(2026-10-23 사람 "링크로 제공이 안됨"). 경로를 링크로 바꾸지 않고 거절한다: 사람이 읽을 요약이 없기 때문이다.
            let relations: Relations = call("status", &(), None)?;
            if relations.parent.is_none() {
                return Err(format!(
                    "You have no parent, so the human reads this report in the inbox and cannot open a path in its text. Write a summary and attach the file as a link: `loom done \"<what you did>\" --doc {} --asked \"<the human's instruction>\"`.",
                    path.display()
                ));
            }
            crate::wsl::for_caller(&path)
        }
        None => summary.unwrap_or_default(),
    };
    let images = images
        .iter()
        .map(|path| absolute(path))
        .collect::<Result<Vec<_>, _>>()?;
    let doc = doc.as_deref().map(absolute).transpose()?;
    let alive_children: Vec<String> = call(
        "done",
        &DoneRequest {
            summary,
            asked,
            images,
            doc,
        },
        None,
    )?;
    println!("Reported to your parent, or to the human's inbox if you have no parent.");
    if !alive_children.is_empty() {
        println!(
            "Warning: these children are still running: {}. Reclaim each with `loom rm <name>` when you keep its results, or they stay in the graph.",
            alive_children.join(", ")
        );
    }
    Ok(())
}

pub fn ready(branch: String) -> Result<(), String> {
    let readied: Readied = call(
        "ready",
        &BranchRequest {
            branch: branch.clone(),
        },
        None,
    )?;
    println!(
        "Sent {branch} to its gate {} (target {}). Do not commit to {branch} until the verdict comes.",
        readied.gate, readied.target
    );
    Ok(())
}

pub fn verdict(
    branch: String,
    judgment: Judgment,
    findings: Option<PathBuf>,
) -> Result<(), String> {
    let findings = findings
        .as_deref()
        .map(absolute)
        .transpose()?
        .map(|path| crate::wsl::for_caller(&path));
    let state: GateState = call(
        "verdict",
        &VerdictRequest {
            branch: branch.clone(),
            judgment,
            findings,
        },
        None,
    )?;
    println!("Recorded the verdict. {branch} is now {state:?}. loom told the receivers.");
    Ok(())
}

pub fn fail(branch: String, cause: String) -> Result<(), String> {
    let state: GateState = call(
        "fail",
        &FailRequest {
            branch: branch.clone(),
            cause,
        },
        None,
    )?;
    println!("{branch} is now {state:?}. loom told the root and the owner.");
    Ok(())
}

/// 관문이 부른다. 저장소의 병합 잠금을 잡고, 데몬이 audit를 하고 할 일을 주면, 이 프로세스가 워크트리 옆에서 검사·병합한다.
/// 결과는 데몬에 알린다. 담당이 고칠 일이나 실패로 끝나면 0이 아닌 코드로 끝난다.
pub fn merge(branch: String) -> Result<(), String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let _lock = MergeLock::take(&here.to_string_lossy())?;
    let plan: MergePlan = call(
        "merge",
        &BranchRequest {
            branch: branch.clone(),
        },
        None,
    )?;
    let (worktree, head, run, into) = match plan {
        MergePlan::Stopped { end, findings } => {
            return Err(format!(
                "Did not merge {branch}. The audit ended it as {end:?} and loom told the receivers:\n- {}",
                findings.join("\n- ")
            ));
        }
        MergePlan::Recorded { sha } => {
            println!(
                "An earlier run already merged {branch} as {sha}, and its report did not arrive. loom recorded it as merged and told the receivers."
            );
            return Ok(());
        }
        MergePlan::Go {
            worktree,
            head,
            run,
            into,
        } => (worktree, head, run, into),
    };
    let log = merge_log(&worktree, &branch)?;
    let result = run_merge(&branch, &worktree, &head, into, &log);
    let request = MergedRequest {
        branch: branch.clone(),
        run,
        result: result.clone(),
    };
    // 배포가 사람의 데몬을 다시 띄운다(host.sh). 끝 보고를 잃지 않게 다시 붙는다(검토 P1-2).
    let state: GateState = reconnecting(None, |_| call("merged", &request, None))?;
    let shown = crate::wsl::for_caller(&log);
    match result {
        MergeResult::Merged { sha, deploy, .. } => {
            println!(
                "Merged {branch} as {sha} (deploy: {deploy:?}). {branch} is now {state:?}. Log: {shown}"
            );
            Ok(())
        }
        MergeResult::Rework { cause, .. } | MergeResult::Failed { cause, .. } => Err(format!(
            "Did not merge {branch}: {cause}. {branch} is now {state:?}, and loom told the receivers. Log: {shown}"
        )),
    }
}

/// 저장소 하나의 `loom merge`는 한 번에 하나만 돈다. 잠금은 공유 git 폴더의 `loom-merge.lock`에 `flock`으로 잡는다.
/// 잡은 `flock`은 이 프로세스의 표준 입력이 닫힐 때까지 산다. 이 프로세스가 끝나거나 죽으면 풀린다(검토 P2-4).
/// WSL 저장소에서는 배포판 안의 `flock`이다. Windows `loom.exe` 여럿과 Linux `loom`이 같은 잠금을 본다.
struct MergeLock(std::process::Child);

impl MergeLock {
    const SCRIPT: &str = r#"exec flock -n "$(git rev-parse --path-format=absolute --git-common-dir)/loom-merge.lock" sh -c 'echo locked; exec cat >/dev/null'"#;
    /// `flock -n`이 잠금을 못 잡으면 1로 끝난다.
    const BUSY: i32 = 1;

    fn take(dir: &str) -> Result<Self, String> {
        let mut cmd = crate::wsl::command(dir, "sh");
        cmd.args(["-c", Self::SCRIPT])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not take the merge lock: {e}"))?;
        let mut line = String::new();
        if let Some(out) = child.stdout.take() {
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(out), &mut line);
        }
        if line.trim() == "locked" {
            return Ok(MergeLock(child));
        }
        drop(child.stdin.take());
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        match out.status.code() {
            Some(Self::BUSY) => Err(
                "Another `loom merge` runs in this repository now. Call it again after that run ends.".into(),
            ),
            _ => Err(format!(
                "could not take the merge lock: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        }
    }
}

impl Drop for MergeLock {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

/// `<공유 git 폴더>/loom-gate/<브랜치>-merge-<유닉스 초>.log`. 워크트리 안에 두면 미커밋 변경이 된다.
/// git 폴더는 WSL 에이전트와 Windows `loom.exe`가 모두 읽고 쓴다(홈 폴더는 둘이 다르다).
fn merge_log(worktree: &str, branch: &str) -> Result<PathBuf, String> {
    let mut common = crate::wsl::command(worktree, "git");
    common.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let common = output(&mut common)?;
    let dir = crate::wsl::from_caller(PathBuf::from(common.trim())).join("loom-gate");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(dir.join(format!("{}-merge-{now}.log", branch.replace('/', "-"))))
}

fn run_merge(branch: &str, worktree: &str, head: &str, into: MergeInto, log: &Path) -> MergeResult {
    let shown = crate::wsl::for_caller(log);
    let failed = |cause: String| MergeResult::Failed {
        cause,
        log: shown.clone(),
    };
    let rework = |cause: String| MergeResult::Rework {
        cause,
        log: shown.clone(),
    };
    // 검사한 글이 병합할 커밋이어야 한다. 검사 앞뒤로 워크트리가 그 커밋에 깨끗이 있는지 본다(검토 P2-2).
    let at_head = |when: &str| -> Result<(), MergeResult> {
        match worktree_at(worktree) {
            Ok((now, clean)) if now == head && clean => Ok(()),
            Ok((now, _)) => Err(rework(format!(
                "{when} the checks, the worktree of {branch} was not clean at the reviewed commit {head} (HEAD {now}). Commit nothing during the gate; call `loom ready` again"
            ))),
            Err(e) => Err(failed(e)),
        }
    };
    if let Err(result) = at_head("before") {
        return result;
    }
    let mut check = crate::wsl::command(worktree, "bash");
    check.arg(protocol::CHECK_SCRIPT);
    match logged(&mut check, log) {
        Ok(0) => {}
        Ok(code) => return rework(format!("{} exited {code}", protocol::CHECK_SCRIPT)),
        Err(e) => return failed(e),
    }
    if let Err(result) = at_head("after") {
        return result;
    }
    match into {
        MergeInto::Main { checkout } => {
            let mut ship = crate::wsl::command(&checkout, "bash");
            ship.args([protocol::SHIP_SCRIPT, branch, head]);
            let deploy = match logged(&mut ship, log) {
                Ok(0) => Deploy::Confirmed,
                Ok(protocol::SHIP_DEPLOY_FAILED) => Deploy::Failed,
                Ok(protocol::SHIP_REWORK) => {
                    return rework(format!(
                        "{} found an owner-fixable problem (conflict, or the target moved)",
                        protocol::SHIP_SCRIPT
                    ));
                }
                Ok(code) => return failed(format!("{} exited {code}", protocol::SHIP_SCRIPT)),
                Err(e) => return failed(e),
            };
            match last_commit(log) {
                Some(sha) => MergeResult::Merged {
                    sha,
                    deploy,
                    log: shown.clone(),
                },
                None => failed(format!(
                    "{} did not print the merged commit on its last line",
                    protocol::SHIP_SCRIPT
                )),
            }
        }
        MergeInto::Branch { worktree: target } => {
            let message = format!("Merge branch '{branch}'");
            let mut merge = crate::wsl::command(&target, "git");
            merge.args(["merge", "--no-ff", "-m", &message, head]);
            match logged(&mut merge, log) {
                Ok(0) => {}
                Ok(_) => {
                    let mut abort = crate::wsl::command(&target, "git");
                    abort.args(["merge", "--abort"]);
                    let _ = logged(&mut abort, log);
                    return rework(format!("git merge into {target} failed"));
                }
                Err(e) => return failed(e),
            }
            let mut sha = crate::wsl::command(&target, "git");
            sha.args(["rev-parse", "HEAD"]);
            match output(&mut sha) {
                Ok(sha) => MergeResult::Merged {
                    sha: sha.trim().to_owned(),
                    deploy: Deploy::NotApplicable,
                    log: shown.clone(),
                },
                Err(e) => failed(e),
            }
        }
    }
}

/// 워크트리의 HEAD 커밋과 미커밋 변경이 없는지.
fn worktree_at(worktree: &str) -> Result<(String, bool), String> {
    let mut head = crate::wsl::command(worktree, "git");
    head.args(["rev-parse", "HEAD"]);
    let mut status = crate::wsl::command(worktree, "git");
    status.args(["status", "--porcelain"]);
    Ok((
        output(&mut head)?.trim().to_owned(),
        output(&mut status)?.trim().is_empty(),
    ))
}

/// 명령의 표준 출력. 0이 아니게 끝나면 표준 오류가 오류다.
fn output(cmd: &mut std::process::Command) -> Result<String, String> {
    let out = cmd
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run {cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 명령의 표준 출력과 오류를 `log` 끝에 붙이고 끝 코드를 돌려준다. 신호로 끝나면 -1이다.
fn logged(cmd: &mut std::process::Command, log: &Path) -> Result<i32, String> {
    let open = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .map_err(|e| format!("{}: {e}", log.display()))
    };
    let status = cmd
        .stdin(std::process::Stdio::null())
        .stdout(open()?)
        .stderr(open()?)
        .status()
        .map_err(|e| format!("could not run {cmd:?}: {e}"))?;
    Ok(status.code().unwrap_or(-1))
}

/// 로그 마지막 줄이 커밋(16진 7자 이상)이면 그것.
fn last_commit(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    (last.len() >= 7 && last.chars().all(|c| c.is_ascii_hexdigit())).then(|| last.to_owned())
}

/// 데몬이 다른 폴더에서 읽으므로 절대 경로로 보낸다. 파일이 없으면 여기서 실패한다.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))
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

pub fn relaunch(name: Option<String>) -> Result<(), String> {
    let yourself = name.is_none();
    let when: Relaunched = call("relaunch", &RelaunchRequest { name }, None)?;
    println!(
        "{}",
        match (when, yourself) {
            (Relaunched::AtTurnEnd, true) => {
                "You relaunch when this turn ends, and the conversation continues. End your turn now."
            }
            (Relaunched::AtTurnEnd, false) => "That node relaunches when its turn ends.",
            (Relaunched::Now, _) => "Relaunched.",
        }
    );
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

/// 흔적의 입구: 이 프로세스를 띄운 `loom` 하위 명령.
fn tool() -> String {
    let verb = std::env::args().nth(1).unwrap_or_default();
    format!("loom {verb}")
}

fn version() -> String {
    match crate::COMMIT {
        Some(commit) => format!("{} ({commit})", crate::VERSION),
        None => crate::VERSION.to_owned(),
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
        .header(TOOL_HEADER, tool())
        .header(VERSION_HEADER, version())
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
