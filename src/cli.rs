use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::daemon::gate::{
    FailRequest, Readied, ReadyRequest, TOOL_HEADER, VERSION_HEADER, VerdictRequest,
};
use crate::daemon::{
    AckRequest, AdoptRequest, AnswerRequest, AnsweredRequest, AskRequest, AskResult, DoneRequest,
    GuardRequest, HookWaitRequest, IdleRequest, NewAgent, NoteRequest, RelaunchRequest, Relaunched,
    RemoveRequest, Reported, SaidRequest, SpawnRequest, SpawnRun, TakeRequest, Taken,
    TurnEndRequest, TurnId, WaitRequest,
};
use crate::graph::{Listed, NodeId, Relations, Sent};
use crate::profile::claude;
use crate::profile::{Take, cursor, pi};
use crate::protocol::{
    AgentId, Delivery, GateState, Judgment, Kind, MessageId, ModelChoice, WaitId,
};
use crate::roles::{Roles, SetRole};
use crate::task;
use crate::wsl::REWAKE_EXIT;

const WAIT_RESPONSE_GRACE_SECS: u64 = 5;
const RECONNECT_AFTER: Duration = Duration::from_secs(2);
const RECONNECT_FOR: Duration = Duration::from_secs(10 * 60);

pub fn spawn(run: SpawnRun, worktree: Option<String>, file: Option<PathBuf>) -> Result<(), String> {
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
    let request = SpawnRequest {
        run,
        brief,
        worktree,
    };
    let _: NodeId = post(&env_var("LOOM_SPAWN_URL")?, None, "spawn", &request, None)?;
    println!("Created node: {}", request.run.name());
    Ok(())
}

pub fn roles() -> Result<(), String> {
    print_roles(call("roles", &(), None)?);
    Ok(())
}

pub fn set_role(set: SetRole) -> Result<(), String> {
    print_roles(call("roles/set", &set, None)?);
    Ok(())
}

fn print_roles(roles: Roles) {
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
    let hook = claude::Hook::deserialize(&input).map_err(|e| format!("hook input: {e}"))?;
    let event = &hook.event;
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    if let Some(model) = claude::reported_model(event, &input)
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    let request = HookWaitRequest {
        wait: event.human_wait(),
    };
    if hook.reports_wait()
        && let Err(e) = call::<()>("hook-wait", &request, Some(timeout))
    {
        eprintln!("loom: could not report the human wait: {e}");
    }
    let transcript = input["transcript_path"].as_str().map(str::to_owned);
    let kinds = match hook.take() {
        Take::Now(kinds) => kinds,
        Take::Never => {
            let reply = match hook.asks_interrupt() && interrupt_pending(timeout) {
                true => Some(claude::subagent_interrupted()),
                false => event.reply(&[], None),
            };
            if let Some(reply) = reply {
                println!("{reply}");
            }
            return Ok(());
        }
        Take::WhenIdle => {
            if let claude::Event::Stop = event
                && let Some(nudge) = turn_end(claude::last_reply(&input), None, timeout)
            {
                to_model(&nudge)?;
                std::process::exit(REWAKE_EXIT)
            }
            let taken = wait_for_mail(&IdleRequest {
                transcript,
                rate_limited: event.rate_limited(),
                turn: None,
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
        hook: hook_name(&input),
        kinds: kinds.to_vec(),
        limit_bytes: claude::HOOK_LIMIT_BYTES,
        transcript,
        human_input: event.human_input(),
        prompt: event.prompt().map(str::to_owned),
        turn: None,
    };
    let taken: Taken = call("take", &request, Some(timeout))?;
    if let Some(reply) = event.reply(&taken.messages, taken.note.as_deref()) {
        println!("{reply}");
    }
    acknowledge(&taken.messages, Some(timeout));
    Ok(())
}

fn interrupt_pending(timeout: Duration) -> bool {
    call("interrupt-pending", &(), Some(timeout)).unwrap_or_else(|e| {
        eprintln!("loom: could not check for an interrupt: {e}");
        false
    })
}

fn hook_name(input: &serde_json::Value) -> String {
    input["hook_event_name"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn to_model(text: &str) -> Result<(), String> {
    let mut stderr = std::io::stderr();
    writeln!(stderr, "{text}").map_err(|e| e.to_string())?;
    stderr.flush().map_err(|e| e.to_string())
}

fn turn_end(reply: Option<String>, turn: Option<TurnId>, timeout: Duration) -> Option<String> {
    if let Some(text) = reply
        && let Err(e) = call::<()>("said", &SaidRequest { text, turn: None }, Some(timeout))
    {
        eprintln!("loom: could not report the reply: {e}");
    }
    call::<Option<String>>("turn-end", &TurnEndRequest { turn }, Some(timeout)).unwrap_or_else(
        |e| {
            eprintln!("loom: could not check the turn end: {e}");
            None
        },
    )
}

pub fn cursor_hook() -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("hook input: {e}"))?;
    let event = cursor::Event::deserialize(&input).map_err(|e| format!("hook input: {e}"))?;
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    let transcript = input["transcript_path"].as_str().map(str::to_owned);
    let turn = input["generation_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(|id| TurnId(id.to_owned()));
    if let Some(model) = event.model()
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    let take_now = |kinds: &[Kind]| {
        call(
            "take",
            &TakeRequest {
                hook: hook_name(&input),
                kinds: kinds.to_vec(),
                limit_bytes: cursor::HOOK_LIMIT_BYTES,
                transcript: transcript.clone(),
                human_input: matches!(event, cursor::Event::BeforeSubmitPrompt { .. }),
                prompt: None,
                turn: turn.clone(),
            },
            Some(timeout),
        )
    };
    if let cursor::Event::AfterAgentResponse { text } = &event
        && let Err(e) = call::<()>(
            "said",
            &SaidRequest {
                text: text.clone(),
                turn: turn.clone(),
            },
            Some(timeout),
        )
    {
        eprintln!("loom: could not report the reply: {e}");
    }
    if let cursor::Event::Stop { .. } = event {
        let here = std::env::current_dir().unwrap_or_default();
        let file = transcript
            .as_deref()
            .and_then(|path| std::fs::File::open(crate::wsl::from_node(path.into(), &here)).ok());
        match file.map(|file| cursor::used_question_box(std::io::BufReader::new(file))) {
            Some(Ok(true)) => {
                if let Err(e) = call::<()>("question-box", &(), Some(timeout)) {
                    eprintln!("loom: could not report the question box: {e}");
                }
            }
            Some(Err(e)) => eprintln!("loom: could not read the transcript: {e}"),
            None | Some(Ok(false)) => {}
        }
        if let Some(nudge) = turn_end(None, turn.clone(), timeout) {
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
            messages: wait_until_mail(&IdleRequest {
                transcript,
                rate_limited: false,
                turn: turn.clone(),
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

pub fn pi_hook() -> Result<(), String> {
    let out = std::sync::Arc::new(std::sync::Mutex::new(std::io::stdout()));
    for line in std::io::stdin().lines() {
        let line = line.map_err(|e| format!("hook input: {e}"))?;
        let request = match pi::request(&line) {
            Ok(request) => request,
            Err(unserved) => return pi_unserved(unserved, &out),
        };
        let out = out.clone();
        let id = request.id;
        let send = move |reply: &pi::Reply| pi_send(&out, reply);
        if id.is_none() {
            return pi_event(request.event, id, send);
        }
        std::thread::spawn(move || {
            if let Err(e) = pi_event(request.event, id, &send) {
                let _ = send(&pi::failed(id, e));
            }
        });
    }
    Ok(())
}

fn pi_unserved(
    unserved: pi::Unserved,
    out: &std::sync::Mutex<std::io::Stdout>,
) -> Result<(), String> {
    if unserved.id.is_some() {
        pi_send(out, &pi::failed(unserved.id, unserved.error.clone()))?;
    }
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    let relaunch = match call::<Relaunched>("relaunch-unserved", &(), Some(timeout)) {
        Ok(_) => "loom relaunches this node with the current pi extension".to_owned(),
        Err(e) => format!("loom could not relaunch this node: {e}"),
    };
    Err(format!("{}. {relaunch}.", unserved.error))
}

fn pi_send(out: &std::sync::Mutex<std::io::Stdout>, reply: &pi::Reply) -> Result<(), String> {
    let mut out = out.lock().unwrap();
    serde_json::to_writer(&mut *out, reply).map_err(|e| e.to_string())?;
    writeln!(out)
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

fn pi_event(
    event: pi::Event,
    id: Option<u64>,
    send: impl Fn(&pi::Reply) -> Result<(), String>,
) -> Result<(), String> {
    let timeout = Duration::from_secs(claude::HOOK_HTTP_TIMEOUT_SECS);
    let transcript = event.transcript();
    if let Some(model) = event.model()
        && let Err(e) = call::<()>("model", &model, Some(timeout))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    if let Some(reply) = event.turn_ends()
        && let Some(nudge) = turn_end(reply, None, timeout)
    {
        return send(&pi::nudge(id, nudge));
    }
    let taken = match event.take() {
        None | Some(Take::Never) => Taken {
            messages: Vec::new(),
            note: None,
        },
        Some(Take::WhenIdle) => Taken {
            messages: wait_until_mail(&IdleRequest {
                transcript,
                rate_limited: false,
                turn: None,
            })?,
            note: None,
        },
        Some(Take::Now(kinds)) => call(
            "take",
            &TakeRequest {
                hook: event.name().to_owned(),
                kinds: kinds.to_vec(),
                limit_bytes: pi::HOOK_LIMIT_BYTES,
                transcript,
                human_input: event.prompt().is_some(),
                prompt: event.prompt().map(str::to_owned),
                turn: None,
            },
            Some(timeout),
        )?,
    };
    send(&pi::reply(id, &taken.messages, taken.note.as_deref()))?;
    acknowledge(&taken.messages, Some(timeout));
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

fn wait_until_mail(request: &IdleRequest) -> Result<Vec<Delivery>, String> {
    until_answered(RECONNECT_AFTER, || call("idle", request, None))
}

fn until_answered<T>(
    pause: Duration,
    mut attempt: impl FnMut() -> Result<T, CallError>,
) -> Result<T, String> {
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(CallError::Unreachable(_)) => std::thread::sleep(pause),
            Err(e) => return Err(e.into()),
        }
    }
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
        "Sent {branch} to its gate {} (target {}). A branch made from its parent's branch: that parent accepts it with `loom verdict`. A branch into the original checkout: loom sends this round to the reviewers the branch has, and the gate spawns one only for a role it has none of. On pass, loom queues, checks and merges the branch and sends the result. Do not commit to {branch} until the result comes.",
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
    let path = task_dir()?.join(task::findings(branch, &relations.me.name));
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
    println!("{branch} is now {state:?}. loom told the receivers.");
    Ok(())
}

fn task_dir() -> Result<PathBuf, String> {
    let dir: Option<String> = call("task", &(), None)?;
    dir.map(|d| crate::wsl::from_caller(PathBuf::from(d)))
        .ok_or_else(|| "loom has no task folder for this node".to_owned())
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
    match crate::protocol::COMMIT {
        Some(commit) => format!("{} ({commit})", crate::protocol::VERSION),
        None => crate::protocol::VERSION.to_owned(),
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

    #[test]
    fn the_idle_wait_outlasts_any_outage_and_ends_only_on_mail_or_refusal() {
        let mut calls = 0;
        let got = until_answered(Duration::ZERO, || {
            calls += 1;
            if calls <= 10_000 {
                Err(unreachable())
            } else {
                Ok("mail")
            }
        });
        assert_eq!(got, Ok("mail"));
        let got: Result<(), String> =
            until_answered(Duration::ZERO, || Err(CallError::Refused("gone".into())));
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
}
