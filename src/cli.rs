use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::daemon::gate::{
    BranchRequest, Deploy, FailRequest, MergeInto, MergePlan, MergeResult, MergedRequest, Readied,
    ReadyRequest, TOOL_HEADER, VERSION_HEADER, VerdictRequest,
};
use crate::daemon::{
    AckRequest, AdoptRequest, AnswerRequest, AnsweredRequest, AskRequest, AskResult, DoneRequest,
    GuardRequest, HookWaitRequest, IdleRequest, NewAgent, NoteRequest, RelaunchRequest, Relaunched,
    RemoveRequest, Reported, SaidRequest, SpawnRequest, SpawnRun, TakeRequest, Taken, TaskRequest,
    WaitRequest,
};
use crate::graph::{Delivery, Kind, Listed, MessageId, NodeId, Relations, Sent, WaitId};
use crate::profile::claude;
use crate::profile::{AgentId, ModelChoice};
use crate::profile::{Take, cursor};
use crate::protocol::{self, GateState, Judgment};
use crate::roles::Roles;
use crate::task;

const WAIT_RESPONSE_GRACE_SECS: u64 = 5;
const RECONNECT_AFTER: Duration = Duration::from_secs(2);
const RECONNECT_FOR: Duration = Duration::from_secs(10 * 60);
const REWAKE_EXIT: i32 = 2;

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

pub fn note(to: String, mode: Kind, body: String) -> Result<(), String> {
    let sent: Sent = call("note", &NoteRequest { to, mode, body }, None)?;
    print_sent(&sent);
    Ok(())
}

pub fn answer(message: u32, body: String) -> Result<(), String> {
    let sent: Sent = call(
        "answer",
        &AnswerRequest {
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
        prompt: event.prompt().map(str::to_owned),
    };
    let taken: Taken = call("take", &request, Some(timeout))?;
    if let Some(reply) = event.reply(&taken.messages, taken.note.as_deref()) {
        println!("{reply}");
    }
    acknowledge(&taken.messages, Some(timeout));
    Ok(())
}

fn to_model(text: &str) -> Result<(), String> {
    let mut stderr = std::io::stderr();
    writeln!(stderr, "{text}").map_err(|e| e.to_string())?;
    stderr.flush().map_err(|e| e.to_string())
}

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
                prompt: None,
            },
            Some(timeout),
        )
    };
    if let cursor::Event::AfterAgentResponse { text } = &event
        && let Err(e) = call::<()>("said", &SaidRequest { text: text.clone() }, Some(timeout))
    {
        eprintln!("loom: could not report the reply: {e}");
    }
    if let cursor::Event::Stop { .. } = event {
        let here = std::env::current_dir().unwrap_or_default();
        let tail = transcript.as_deref().and_then(|path| {
            crate::profile::transcript_tail(&crate::wsl::from_node(path.into(), &here))
        });
        if tail.is_some_and(|text| cursor::used_question_box(&text))
            && let Err(e) = call::<()>("question-box", &(), Some(timeout))
        {
            eprintln!("loom: could not report the question box: {e}");
        }
        if let Some(nudge) = turn_end(None, timeout) {
            println!("{}", serde_json::json!({ "followup_message": nudge }));
            return Ok(());
        }
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

pub fn cursor_rules() -> Result<(), String> {
    let env = |key| std::env::var(key).ok();
    let launch = crate::skills::Launch {
        position: cursor::position_from(env(cursor::POSITION_ENV).as_deref()),
        skills: cursor::skills_from(env(cursor::SKILLS_ENV).as_deref()),
    };
    println!("{}", cursor::rules(launch));
    Ok(())
}

fn acknowledge(taken: &[Delivery], timeout: Option<Duration>) {
    if taken.is_empty() {
        return;
    }
    let ids = taken.iter().map(|d| d.id).collect();
    if let Err(e) = call::<()>("ack", &AckRequest { ids }, timeout) {
        eprintln!("loom: could not acknowledge: {e}");
    }
}

fn wait_for_mail(request: &IdleRequest) -> Result<Vec<Delivery>, String> {
    reconnecting(None, |_| call("idle", request, None))
}

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
                if started.elapsed() >= RECONNECT_AFTER {
                    unreachable_since = None;
                }
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

pub fn guard() -> Result<(), String> {
    let caller = std::process::id();
    Ok(call::<()>("guard", &GuardRequest { caller }, None)?)
}

pub fn ask(question: String, doc: Option<PathBuf>) -> Result<(), String> {
    let doc = doc.as_deref().map(absolute).transpose()?;
    let asked = match call(
        "ask",
        &AskRequest {
            question: with_doc_link(&question, doc.as_deref()),
            doc,
        },
        None,
    )? {
        AskResult::Inbox(asked) => asked,
        AskResult::Parent(sent) => {
            print_sent(&sent);
            println!(
                "Asked your parent. The answer comes as a message (kind: answer), or wait for it: `loom wait --reply-to {} --timeout <seconds> --json`.",
                sent.id.0
            );
            return Ok(());
        }
    };
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

pub fn done(
    summary: Option<String>,
    file: Option<PathBuf>,
    asked: Option<String>,
    images: Vec<PathBuf>,
    doc: Option<PathBuf>,
) -> Result<(), String> {
    let file = match file {
        Some(path) => {
            let path = match path.is_relative() {
                true => absolute(&task_dir()?.join(path))?,
                false => absolute(&path)?,
            };
            let relations: Relations = call("status", &(), None)?;
            if relations.parent.is_none() {
                return Err(format!(
                    "You have no parent, so the human reads this report in the inbox and cannot open {} in its text. Write a summary: `loom done \"<what you did>\" --asked \"<the human's instruction>\"`. To show a document, write it as user-doc HTML, run show.sh on it, and add --doc <that file>.",
                    path.display()
                ));
            }
            Some(crate::wsl::for_caller(&path))
        }
        None => None,
    };
    let summary = summary.or_else(|| file.clone()).unwrap_or_default();
    let images = images
        .iter()
        .map(|path| absolute(path))
        .collect::<Result<Vec<_>, _>>()?;
    let doc = doc.as_deref().map(absolute).transpose()?;
    let reported: Reported = call(
        "done",
        &DoneRequest {
            summary: with_doc_link(&summary, doc.as_deref()),
            file,
            asked,
            images,
            doc,
        },
        None,
    )?;
    println!("Reported to your parent, or to the human's inbox if you have no parent.");
    if !reported.alive.is_empty() {
        println!(
            "Warning: these children are still running or asleep: {}. Reclaim each with `loom rm <name>` when you keep its results, or they stay in the graph.",
            reported.alive.join(", ")
        );
    }
    if let Some(warning) = reported.warning {
        println!(
            "Warning: {warning}. Your parent sees this warning. When the branch is ready, call `loom ready <branch>`."
        );
    }
    Ok(())
}

pub fn ready(branch: String) -> Result<(), String> {
    let readied: Readied = call(
        "ready",
        &ReadyRequest {
            branch: branch.clone(),
            cwd: crate::wsl::from_caller(std::env::current_dir().map_err(|e| e.to_string())?)
                .display()
                .to_string(),
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
    let findings = match findings {
        Some(path) => Some(absolute(&path)?),
        None => default_findings(&branch, judgment)?,
    }
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

fn default_findings(branch: &str, judgment: Judgment) -> Result<Option<PathBuf>, String> {
    let relations: Relations = call("status", &(), None)?;
    let path = task_dir()?.join(task::REVIEW).join(format!(
        "{}-{}.md",
        task::folder(branch),
        task::folder(&relations.me.name)
    ));
    match (path.is_file(), judgment) {
        (true, _) => absolute(&path).map(Some),
        (false, Judgment::Pass) => Ok(None),
        (false, Judgment::Block) => Err(format!(
            "A block needs a findings file. Write it at {}, or give --findings <file>.",
            crate::wsl::for_caller(&path)
        )),
    }
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

pub fn merge(branch: String) -> Result<(), String> {
    let log = merge_log(&branch)?;
    let unused = || {
        let _ = std::fs::remove_file(&log);
    };
    let plan: MergePlan = call(
        "merge",
        &BranchRequest {
            branch: branch.clone(),
        },
        None,
    )
    .inspect_err(|_| unused())?;
    let (worktree, head, run, into) = match plan {
        MergePlan::Stopped { end, findings } => {
            unused();
            return Err(format!(
                "Did not merge {branch}. The audit ended it as {end:?} and loom told the receivers:\n- {}",
                findings.join("\n- ")
            ));
        }
        MergePlan::Recorded { sha } => {
            unused();
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
    let (dir, ship) = match &into {
        MergeInto::Main { checkout } => (checkout.as_str(), protocol::SHIP_SCRIPT),
        MergeInto::Branch { worktree } => (worktree.as_str(), ""),
    };
    if lock_busy(dir) {
        println!(
            "Waiting for another loom merge in this repository (up to {} minutes).",
            MERGE_LOCK_WAIT_SECS / 60
        );
    }
    let mut work = crate::wsl::command(dir, "bash");
    work.args([
        "-c",
        MERGE_RUN,
        "loom-merge",
        &inside(&worktree),
        &head,
        &branch,
        ship,
        &MERGE_LOCK_WAIT_SECS.to_string(),
    ]);
    let code = logged(&mut work, &log);
    if code == Ok(RUN_ALREADY_MERGED) {
        println!("{branch} is already in its target branch. Asking loom to record that merge.");
        return merge(branch);
    }
    let result = merge_result(code, &into, &log);
    let request = MergedRequest {
        branch: branch.clone(),
        run,
        result: result.clone(),
    };
    let state: GateState = reconnecting(None, |_| call("merged", &request, None))?;
    let shown = crate::wsl::for_caller(&log);
    match result {
        MergeResult::Merged { sha, deploy, .. } => {
            println!(
                "Merged {branch} as {sha} (deploy: {deploy:?}). {branch} is now {state:?}. Log: {shown}"
            );
            Ok(())
        }
        MergeResult::Rework { cause, .. }
        | MergeResult::Failed { cause, .. }
        | MergeResult::Stopped { cause, .. } => Err(format!(
            "Did not merge {branch}: {cause}. {branch} is now {state:?}, and loom told the receivers. Log: {shown}"
        )),
    }
}

const MERGE_RUN: &str = r#"set -u
wt=$1 head=$2 branch=$3 ship=$4 wait=$5
exec 9>>"$(git rev-parse --path-format=absolute --git-common-dir)/loom-merge.lock"
flock -E 75 -w "$wait" 9 || { echo "another loom merge held the lock for $wait seconds"; exit 75; }
if git merge-base --is-ancestor "$head" HEAD; then
  [ -n "$ship" ] && exec bash "$ship" "$branch" "$head"
  exit 15
fi
at() { [ "$(git -C "$wt" rev-parse HEAD)" = "$head" ] && [ -z "$(git -C "$wt" status --porcelain)" ]; }
at || { echo "the worktree of $branch is not clean at the reviewed commit $head"; exit 13; }
# check.sh와 그 자식(cargo, 시험 프로세스)에게는 잠금을 넘기지 않는다. 남은 자식이 잠금을 쥐면 다음 병합이 한 시간 기다린다(검토 P3-e).
(cd "$wt" && bash scripts/gate/check.sh 9>&-) || { echo "scripts/gate/check.sh exited $?"; exit 14; }
at || { echo "the worktree of $branch moved during the checks"; exit 13; }
[ -n "$ship" ] && exec bash "$ship" "$branch" "$head"
git merge --no-ff -m "Merge branch '$branch'" "$head" || { git merge --abort; exit 10; }
git rev-parse HEAD
"#;
const RUN_ALREADY_MERGED: i32 = 15;
const RUN_MOVED: i32 = 13;
const RUN_CHECK_FAILED: i32 = 14;
const RUN_LOCK_TIMEOUT: i32 = 75;
const MERGE_LOCK_WAIT_SECS: u32 = 3600;

fn merge_result(code: Result<i32, String>, into: &MergeInto, log: &Path) -> MergeResult {
    let log_text = crate::wsl::for_caller(log);
    let failed = |cause: String| MergeResult::Failed {
        cause,
        log: log_text.clone(),
    };
    let rework = |cause: String| MergeResult::Rework {
        cause,
        log: log_text.clone(),
    };
    let stopped = |cause: String| MergeResult::Stopped {
        cause,
        log: log_text.clone(),
    };
    let merged = |deploy| match last_commit(log) {
        Some(sha) => MergeResult::Merged {
            sha,
            deploy,
            log: log_text.clone(),
        },
        None => failed("the merge did not print the merged commit on its last line".into()),
    };
    let main = matches!(into, MergeInto::Main { .. });
    match code {
        Err(e) => failed(e),
        Ok(0) if main => merged(Deploy::Confirmed),
        Ok(0) => merged(Deploy::NotApplicable),
        Ok(protocol::SHIP_DEPLOY_FAILED) if main => merged(Deploy::Failed),
        Ok(protocol::SHIP_REWORK) => rework(
            "the merge found an owner-fixable problem (conflict, or the target moved)".into(),
        ),
        Ok(protocol::SHIP_STOPPED) if main => stopped(format!(
            "{} stopped without an unpublished merge (see the last lines of the log); call `loom merge` again, or tell the root",
            protocol::SHIP_SCRIPT
        )),
        Ok(RUN_MOVED) => rework(
            "the worktree was not clean at the reviewed commit. Commit nothing during the gate; call `loom ready` again".into(),
        ),
        Ok(RUN_CHECK_FAILED) => rework(format!("{} failed", protocol::CHECK_SCRIPT)),
        Ok(RUN_LOCK_TIMEOUT) => stopped(format!(
            "another loom merge held the repository lock for {} minutes; call `loom merge` again later",
            MERGE_LOCK_WAIT_SECS / 60
        )),
        Ok(code) => failed(format!("the merge exited {code}")),
    }
}

fn lock_busy(dir: &str) -> bool {
    let mut probe = crate::wsl::command(dir, "sh");
    probe.args([
        "-c",
        r#"exec flock -n "$(git rev-parse --path-format=absolute --git-common-dir)/loom-merge.lock" true"#,
    ]);
    output(&mut probe).is_err()
}

fn inside(path: &str) -> String {
    crate::wsl::location(path).map_or_else(|| path.to_owned(), |w| w.path)
}

fn merge_log(branch: &str) -> Result<PathBuf, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let log = match branch_task(branch)? {
        Some(dir) => dir.join(task::LOGS).join(format!("merge-{now}.log")),
        None => {
            let here = crate::wsl::from_caller(std::env::current_dir().map_err(|e| e.to_string())?);
            let mut common = crate::wsl::command(&here.display().to_string(), "git");
            common.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
            let common = output(&mut common)?;
            crate::wsl::from_caller(PathBuf::from(common.trim()))
                .join("loom-gate")
                .join(format!("{}-merge-{now}.log", task::folder(branch)))
        }
    };
    create_log(&log)
}

fn create_log(log: &Path) -> Result<PathBuf, String> {
    let dir = log.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stem = log.with_extension("");
    for n in 1.. {
        let path = match n {
            1 => log.to_path_buf(),
            n => PathBuf::from(format!("{}-{n}.log", stem.display())),
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Did not start the merge: cannot write its log {}: {e}",
                    path.display()
                ));
            }
        }
    }
    unreachable!("1.. does not end")
}

fn task_dir() -> Result<PathBuf, String> {
    let dir: Option<String> = call("task", &TaskRequest { branch: None }, None)?;
    dir.map(|d| crate::wsl::from_caller(PathBuf::from(d)))
        .ok_or_else(|| "loom has no task folder for this node".to_owned())
}

fn branch_task(branch: &str) -> Result<Option<PathBuf>, String> {
    let dir: Option<String> = call(
        "task",
        &TaskRequest {
            branch: Some(branch.to_owned()),
        },
        None,
    )?;
    Ok(dir.map(|d| crate::wsl::from_caller(PathBuf::from(d))))
}

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

fn last_commit(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    (last.len() >= 7 && last.chars().all(|c| c.is_ascii_hexdigit())).then(|| last.to_owned())
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn with_doc_link(text: &str, doc: Option<&Path>) -> String {
    doc.map_or_else(
        || text.to_owned(),
        |doc| crate::user_doc::inline_link(text, doc),
    )
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

pub fn relaunch(
    name: Option<String>,
    agent: Option<String>,
    choice: ModelChoice,
) -> Result<(), String> {
    let yourself = name.is_none();
    let to = agent.map(|agent| NewAgent {
        agent: AgentId(agent),
        choice,
    });
    let switch = to.is_some();
    let when: Relaunched = call("relaunch", &RelaunchRequest { name, to }, None)?;
    println!(
        "{}",
        match (when, yourself, switch) {
            (Relaunched::AtTurnEnd, true, false) => {
                "You relaunch when this turn ends, and the conversation continues. End your turn now."
            }
            (Relaunched::AtTurnEnd, true, true) => {
                "The new agent takes over this node when this turn ends. End your turn now."
            }
            (Relaunched::AtTurnEnd, false, _) => "That node relaunches when its turn ends.",
            (Relaunched::Now, _, _) => "Relaunched.",
        }
    );
    Ok(())
}

fn call<T: DeserializeOwned>(
    action: &str,
    body: &impl Serialize,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let url = env_var("LOOM_URL").map_err(CallError::Refused)?;
    post(&url, ureq::Proxy::try_from_env(), action, body, timeout)
}

enum CallError {
    Unreachable(String),
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
        .map_err(|e| {
            CallError::Unreachable(format!("Could not connect to the daemon: {e}. In a sandbox, get approval to run outside the sandbox and try again."))
        })?;
    let status = res.status();
    let text = res.body_mut().read_to_string().map_err(|e| {
        CallError::Unreachable(format!(
            "The daemon closed the connection before its answer was complete: {e}."
        ))
    })?;
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

    #[test]
    fn a_merge_log_never_reuses_a_file_and_a_log_that_cannot_open_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs").join("merge-5.log");
        let first = create_log(&log).unwrap();
        std::fs::write(&first, "earlier merge").unwrap();
        let second = create_log(&log).unwrap();
        assert_eq!(first, log);
        assert_eq!(second, dir.path().join("logs").join("merge-5-2.log"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "earlier merge");
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, "").unwrap();
        assert!(create_log(&blocked.join("merge-5.log")).is_err());
    }

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

    fn answering(answer: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
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

    #[cfg(unix)]
    mod merge_run {
        use super::*;
        use std::process::Command;

        struct Repo {
            _dir: tempfile::TempDir,
            main: PathBuf,
            wt: PathBuf,
            head: String,
        }

        fn git(dir: &Path, args: &[&str]) -> String {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }

        fn repo() -> Repo {
            let dir = tempfile::tempdir().unwrap();
            let main = dir.path().join("main");
            std::fs::create_dir_all(main.join("scripts/gate")).unwrap();
            git(&main, &["init", "-q", "-b", "main"]);
            git(&main, &["config", "user.email", "t@t"]);
            git(&main, &["config", "user.name", "t"]);
            std::fs::write(
                main.join("scripts/gate/check.sh"),
                "[ ! -e check-fails ] && echo checked\n",
            )
            .unwrap();
            std::fs::write(
                main.join("scripts/gate/ship.sh"),
                "echo \"ship $1 $2\"; sleep \"${SHIP_SLEEP:-0}\"; git rev-parse HEAD\n",
            )
            .unwrap();
            git(&main, &["add", "-A"]);
            git(&main, &["commit", "-qm", "init"]);
            let wt = dir.path().join("wt");
            git(
                &main,
                &["worktree", "add", "-q", "-b", "기능", wt.to_str().unwrap()],
            );
            std::fs::write(wt.join("f.txt"), "f").unwrap();
            git(&wt, &["add", "-A"]);
            git(&wt, &["commit", "-qm", "f"]);
            let head = git(&wt, &["rev-parse", "HEAD"]);
            Repo {
                _dir: dir,
                main,
                wt,
                head,
            }
        }

        fn run(r: &Repo, ship: &str, wait: &str) -> (i32, String) {
            let out = Command::new("bash")
                .args(["-c", MERGE_RUN, "loom-merge"])
                .args([r.wt.to_str().unwrap(), &r.head, "기능", ship, wait])
                .current_dir(&r.main)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            (out.status.code().unwrap_or(-1), text)
        }

        fn lock(r: &Repo) -> PathBuf {
            r.main.join(".git/loom-merge.lock")
        }

        #[test]
        fn a_clean_reviewed_commit_is_checked_and_merged_once() {
            let r = repo();
            let (code, text) = run(&r, "", "5");
            assert_eq!(code, 0, "{text}");
            assert!(text.contains("checked"), "{text}");
            let merged = git(&r.main, &["rev-parse", "HEAD"]);
            assert!(text.trim_end().ends_with(&merged), "{text}");
            assert_eq!(git(&r.main, &["rev-parse", "HEAD^2"]), r.head);
            assert_eq!(run(&r, "", "5").0, RUN_ALREADY_MERGED);
            assert_eq!(git(&r.main, &["rev-parse", "HEAD"]), merged);
            let (code, text) = run(&r, "scripts/gate/ship.sh", "5");
            assert_eq!(code, 0, "{text}");
            assert!(text.contains(&format!("ship 기능 {}", r.head)), "{text}");
            assert!(!text.contains("checked"), "no second check: {text}");
        }

        #[test]
        fn a_moved_or_dirty_worktree_and_a_failed_check_are_rework() {
            let r = repo();
            std::fs::write(r.wt.join("dirty.txt"), "x").unwrap();
            assert_eq!(run(&r, "", "5").0, RUN_MOVED);
            std::fs::remove_file(r.wt.join("dirty.txt")).unwrap();
            std::fs::write(r.wt.join("check-fails"), "").unwrap();
            git(&r.wt, &["add", "-A"]);
            git(&r.wt, &["commit", "-qm", "late"]);
            assert_eq!(
                run(&r, "", "5").0,
                RUN_MOVED,
                "HEAD is not the reviewed commit"
            );
            let r = Repo {
                head: git(&r.wt, &["rev-parse", "HEAD"]),
                ..r
            };
            assert_eq!(run(&r, "", "5").0, RUN_CHECK_FAILED);
            assert_eq!(git(&r.main, &["log", "--oneline"]).lines().count(), 1);
        }

        #[test]
        fn a_leftover_check_child_does_not_hold_the_lock() {
            let r = repo();
            std::fs::write(
                r.main.join("scripts/gate/check.sh"),
                "(setsid sleep 5 >/dev/null 2>&1 &) ; echo checked\n",
            )
            .unwrap();
            git(&r.main, &["commit", "-qam", "check leaves a child"]);
            git(&r.wt, &["merge", "-q", "--no-edit", "main"]);
            let r = Repo {
                head: git(&r.wt, &["rev-parse", "HEAD"]),
                ..r
            };
            let (code, text) = run(&r, "", "5");
            assert_eq!(code, 0, "{text}");
            assert!(
                !lock_busy(r.main.to_str().unwrap()),
                "the sleep keeps no lock"
            );
        }

        #[test]
        fn the_lock_waits_then_times_out_and_outlives_a_killed_caller() {
            let r = repo();
            std::fs::File::create(lock(&r)).unwrap();
            let mut holder = Command::new("flock")
                .args([lock(&r).to_str().unwrap(), "sleep", "3"])
                .spawn()
                .unwrap();
            std::thread::sleep(Duration::from_millis(300));
            assert!(
                lock_busy(r.wt.to_str().unwrap()),
                "the worktree shares the lock"
            );
            let (code, text) = run(&r, "", "1");
            assert_eq!(code, RUN_LOCK_TIMEOUT, "{text}");
            holder.wait().unwrap();
            assert!(!lock_busy(r.main.to_str().unwrap()));
            let mut work = Command::new("bash")
                .args(["-c", MERGE_RUN, "loom-merge"])
                .args([
                    r.wt.to_str().unwrap(),
                    &r.head,
                    "기능",
                    "scripts/gate/ship.sh",
                    "5",
                ])
                .env("SHIP_SLEEP", "3")
                .current_dir(&r.main)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            std::thread::sleep(Duration::from_millis(1500));
            assert!(
                lock_busy(r.main.to_str().unwrap()),
                "ship.sh holds the lock"
            );
            work.kill().unwrap();
            work.wait().unwrap();
            assert!(
                lock_busy(r.main.to_str().unwrap()),
                "the child of ship.sh keeps the lock"
            );
        }
    }
}
