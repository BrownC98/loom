use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::daemon::gate::{Readied, ReadyRequest, TOOL_HEADER, VERSION_HEADER, VerdictRequest};
use crate::daemon::{
    AdoptRequest, AnswerRequest, AnsweredRequest, AskRequest, AskResult, DetachRequest,
    DoneRequest, GuardRequest, NewAgent, NoteRequest, RelaunchRequest, Relaunched, RemoveRequest,
    Reported, SpawnRequest, SpawnRun, WaitRequest,
};
use crate::graph::{Kept, Listed, NodeId, Relations, Sent};
use crate::protocol::{
    AckRequest, AgentId, ChoiceRequest, Delivery, GateState, IdleReply, IdleRequest, Judgment,
    Kind, MessageId, ModelChoice, NODE_CALLS, NODE_CALLS_HEADER, Report, WaitId,
};
use crate::roles::{Roles, SetRole};
use crate::task;
use crate::tunnel::ENDPOINT_ENV;

mod endpoint;
mod hook;

pub use hook::hook;

const WAIT_RESPONSE_GRACE_SECS: u64 = 5;
const RECONNECT_AFTER: Duration = Duration::from_secs(2);
const RECONNECT_FOR: Duration = Duration::from_secs(10 * 60);

const NO_BRIEF: &str = "loom spawn: no brief. Write it to a file and give --file <path>.";

fn read_brief(run: &SpawnRun, file: Option<&Path>) -> Result<String, String> {
    let brief = match file {
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => String::new(),
    };
    if brief.trim().is_empty() && matches!(run, SpawnRun::Role { .. }) {
        return Err(NO_BRIEF.into());
    }
    Ok(brief)
}

pub fn spawn(run: SpawnRun, worktree: Option<String>, file: Option<PathBuf>) -> Result<(), String> {
    let brief = read_brief(&run, file.as_deref())?;
    let request = SpawnRequest {
        run,
        brief,
        worktree,
    };
    let spawn = Daemon::Http {
        url: env_var("LOOM_SPAWN_URL")?,
        proxy: None,
    };
    let _: NodeId = post(&spawn, "spawn", &request, None, None)?;
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

pub fn wait(reply_to: Option<u32>, timeout_secs: Option<u64>) -> Result<(), String> {
    let deadline = timeout_secs.map(|secs| Instant::now() + Duration::from_secs(secs));
    let delivery: Delivery = reconnecting(deadline, |remaining| {
        let timeout_secs = remaining.map(|left| left.as_secs());
        let request = WaitRequest {
            reply_to: reply_to.map(MessageId),
            timeout_secs,
        };
        let timeout = timeout_secs
            .map(|secs| Duration::from_secs(secs.saturating_add(WAIT_RESPONSE_GRACE_SECS)));
        call_within("wait", &request, remaining, timeout)
    })?;
    let id = delivery.id;
    println!("{}", crate::profile::message_text(&[delivery]));
    std::io::Write::flush(&mut std::io::stdout()).map_err(|e| e.to_string())?;
    reconnecting(None, |_| {
        call::<()>(
            "ack",
            &AckRequest {
                ids: vec![id],
                handoff: None,
            },
            None,
        )
    })
    .map_err(|e| {
        format!(
            "Could not acknowledge: {e}. Message #{} can come again.",
            id.0
        )
    })
}

fn wait_until_mail(request: IdleRequest) -> Result<IdleReply, String> {
    let idle = || call("idle", &request, None);
    match through_endpoint() {
        true => again_after_cut(None, |_| idle(), |e, _| Err(e.into())),
        false => idle().map_err(Into::into),
    }
}

fn interrupt_arrived(close: endpoint::Close) -> Result<bool, String> {
    let Some(at) = std::env::var_os(ENDPOINT_ENV) else {
        return Ok(false);
    };
    let token = env_var("LOOM_TOKEN")?;
    let endpoint = Daemon::Endpoint(at.into(), Some(close));
    post_as::<()>(&endpoint, &token, "interrupt", &(), None, None)?;
    Ok(true)
}

fn choice_made(asked: &ChoiceRequest, close: endpoint::Close) -> Result<usize, String> {
    let Some(at) = std::env::var_os(ENDPOINT_ENV) else {
        return Err(format!(
            "{ENDPOINT_ENV} is not set, so loom cannot hold a choice"
        ));
    };
    let token = env_var("LOOM_TOKEN")?;
    let endpoint = Daemon::Endpoint(at.into(), Some(close.clone()));
    let ask = || match close.closed() {
        true => Err(CallError::Refused("loom closed this choice".into())),
        false => post_as(&endpoint, &token, "choice", asked, None, None),
    };
    again_after_cut(None, |_| ask(), |e, _| Err(e.into()))
}

fn choice_withdrawn() -> Result<(), String> {
    reconnecting(None, |_| {
        call::<()>("report", &Report::HumanWait(None), None)
    })
}

fn reconnecting<T>(
    deadline: Option<Instant>,
    attempt: impl FnMut(Option<Duration>) -> Result<T, CallError>,
) -> Result<T, String> {
    match through_endpoint() {
        true => {
            let mut outage = Outage::new(RECONNECT_FOR);
            again_after_cut(deadline, attempt, |e, started| {
                outage.pause(e, started, deadline)
            })
        }
        false => reconnecting_within(RECONNECT_FOR, deadline, attempt),
    }
}

fn through_endpoint() -> bool {
    std::env::var_os(ENDPOINT_ENV).is_some()
}

fn again_after_cut<T>(
    deadline: Option<Instant>,
    mut attempt: impl FnMut(Option<Duration>) -> Result<T, CallError>,
    mut through_url: impl FnMut(CallError, Instant) -> Result<(), String>,
) -> Result<T, String> {
    loop {
        let started = Instant::now();
        let left = deadline.map(|d| d.saturating_duration_since(started));
        match attempt(left) {
            Ok(value) => return Ok(value),
            Err(e @ CallError::FellBack(_)) => through_url(e, started)?,
            Err(CallError::Unreachable(_)) if left.is_none_or(|left| !left.is_zero()) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

fn reconnecting_within<T>(
    outage_limit: Duration,
    deadline: Option<Instant>,
    mut attempt: impl FnMut(Option<Duration>) -> Result<T, CallError>,
) -> Result<T, String> {
    let mut outage = Outage::new(outage_limit);
    loop {
        let started = Instant::now();
        match attempt(deadline.map(|d| d.saturating_duration_since(started))) {
            Ok(value) => return Ok(value),
            Err(e) => outage.pause(e, started, deadline)?,
        }
    }
}

struct Outage {
    limit: Duration,
    since: Option<Instant>,
}

impl Outage {
    fn new(limit: Duration) -> Outage {
        Outage { limit, since: None }
    }

    fn pause(
        &mut self,
        error: CallError,
        started: Instant,
        deadline: Option<Instant>,
    ) -> Result<(), String> {
        let error = match error.through_url() {
            error @ (CallError::Unreachable(_)
            | CallError::Failed(_)
            | CallError::NotConnected(_)) => error,
            e => return Err(e.into()),
        };
        if matches!(error, CallError::Unreachable(_) | CallError::Failed(_))
            && started.elapsed() >= RECONNECT_AFTER
        {
            self.since = None;
        }
        let e = error.to_string();
        if self.since.get_or_insert_with(Instant::now).elapsed() >= self.limit {
            return Err(format!(
                "{e} The daemon was down for {} min. A message that was on its way to you comes again.",
                self.limit.as_secs() / 60
            ));
        }
        if deadline.is_some_and(|d| Instant::now() + RECONNECT_AFTER >= d) {
            return Err(e);
        }
        std::thread::sleep(RECONNECT_AFTER);
        Ok(())
    }
}

pub fn guard() -> Result<(), String> {
    let request = match crate::wsl::parent_shell() {
        Some(shell) => GuardRequest::Inside {
            shell: shell.pid,
            birth: shell.birth,
        },
        None => GuardRequest::Pc {
            caller: std::process::id(),
        },
    };
    Ok(call::<()>("guard", &request, None)?)
}

pub fn ask(question: String, doc: Option<PathBuf>) -> Result<(), String> {
    let doc = doc.as_deref().map(absolute).transpose()?;
    let asked = match call(
        "ask",
        &AskRequest {
            question: with_doc_link(&question, doc.as_deref()),
            doc: doc.map(crate::wsl::for_daemon),
        },
        None,
    )? {
        AskResult::Inbox(asked) => asked,
        AskResult::Parent(sent) => {
            print_sent(&sent);
            println!(
                "Asked your parent. The answer comes as a message (kind: answer), or wait for it: `loom wait --reply-to {} --timeout <seconds>`.",
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

pub fn kept(given: &str) -> Result<Kept, String> {
    match given.split_once('=') {
        Some((name, reason)) if !name.trim().is_empty() && !reason.trim().is_empty() => Ok(Kept {
            name: name.trim().to_owned(),
            reason: reason.trim().to_owned(),
        }),
        _ => Err("give <child name>=<reason why you keep it>".to_owned()),
    }
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
    keep: Vec<Kept>,
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
        .map(|path| absolute(path).map(crate::wsl::for_daemon))
        .collect::<Result<Vec<_>, _>>()?;
    let doc = doc.as_deref().map(absolute).transpose()?;
    let reported: Reported = call(
        "done",
        &DoneRequest {
            summary: with_doc_link(&summary, doc.as_deref()),
            file,
            asked,
            images,
            doc: doc.map(crate::wsl::for_daemon),
            keep,
        },
        None,
    )?;
    println!("Reported to your parent, or to the human's inbox if you have no parent.");
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
            cwd: crate::wsl::cwd_for_daemon(std::env::current_dir().map_err(|e| e.to_string())?)
                .display()
                .to_string(),
        },
        None,
    )?;
    println!(
        "Sent {branch} to its gate {} (target {}). A branch made from its parent's branch: that parent accepts it with `loom verdict`. A branch into the original checkout: loom spawns one reviewer per reviewer role as children of the gate, and sends each next round to them. On pass, loom queues, checks and merges the branch and sends the result. To change the branch before the queue takes it, commit and call `loom ready {branch}` again: the new round replaces this one.",
        readied.gate, readied.target
    );
    for problem in &readied.unseated {
        println!(
            "loom could not spawn the reviewer {problem}. Call `loom ready {branch}` again to try again."
        );
    }
    Ok(())
}

pub fn verdict(
    branch: String,
    head: String,
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
            head,
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

fn task_dir() -> Result<PathBuf, String> {
    let dir: Option<String> = call("task", &(), None)?;
    dir.map(crate::wsl::from_daemon)
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

pub fn detach(name: String) -> Result<(), String> {
    call::<()>("detach", &DetachRequest { name: name.clone() }, None)?;
    println!("{name} is a root node now. Its loom ask goes to the human inbox.");
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
    call_within(action, body, timeout, timeout)
}

fn call_within<T: DeserializeOwned>(
    action: &str,
    body: &impl Serialize,
    connect: Option<Duration>,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let token = env_var("LOOM_TOKEN").map_err(CallError::Refused)?;
    let endpoint = std::env::var_os(ENDPOINT_ENV).map(PathBuf::from);
    reach(
        endpoint,
        Daemon::from_env,
        &token,
        action,
        body,
        connect,
        timeout,
    )
}

fn reach<T: DeserializeOwned>(
    endpoint: Option<PathBuf>,
    http: impl FnOnce() -> Result<Daemon, CallError>,
    token: &str,
    action: &str,
    body: &impl Serialize,
    connect: Option<Duration>,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let Some(at) = endpoint else {
        return post_as(&http()?, token, action, body, connect, timeout);
    };
    match post_as(
        &Daemon::Endpoint(at, None),
        token,
        action,
        body,
        connect,
        timeout,
    ) {
        Err(CallError::NotConnected(_)) => {}
        reached => return reached,
    }
    http()
        .and_then(|daemon| post_as(&daemon, token, action, body, connect, timeout))
        .map_err(|e| CallError::FellBack(Box::new(e)))
}

enum Daemon {
    Http {
        url: String,
        proxy: Option<ureq::Proxy>,
    },
    Endpoint(PathBuf, Option<endpoint::Close>),
}

impl Daemon {
    fn from_env() -> Result<Daemon, CallError> {
        Ok(Daemon::Http {
            url: env_var("LOOM_URL").map_err(CallError::Refused)?,
            proxy: ureq::Proxy::try_from_env(),
        })
    }

    fn agent(&self, connect: Option<Duration>, timeout: Option<Duration>) -> (ureq::Agent, &str) {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(connect)
            .timeout_global(timeout);
        match self {
            Daemon::Http { url, proxy } => (config.proxy(proxy.clone()).build().into(), url),
            Daemon::Endpoint(at, close) => (
                ureq::Agent::with_parts(
                    config.build(),
                    endpoint::Connector {
                        at: at.clone(),
                        close: close.clone(),
                    },
                    ureq::unversioned::resolver::DefaultResolver::default(),
                ),
                endpoint::URL,
            ),
        }
    }

    fn not_answered(&self, e: ureq::Error) -> String {
        match (self, e) {
            (Daemon::Http { .. }, e) => format!(
                "Could not connect to the daemon: {e}. In a sandbox, get approval to run outside the sandbox and try again."
            ),
            (Daemon::Endpoint(at, _), ureq::Error::ConnectionFailed) => {
                format!("Could not connect to the node endpoint {}.", at.display())
            }
            (Daemon::Endpoint(at, _), e @ ureq::Error::Timeout(_)) => format!(
                "The daemon did not answer through the node endpoint {} in time: {e}.",
                at.display()
            ),
            (Daemon::Endpoint(at, _), e) => format!(
                "The node endpoint {} closed after loom sent the request: {e}. The daemon may have done it. Look with `loom status` before you send it again.",
                at.display()
            ),
        }
    }
}

enum CallError {
    NotConnected(String),
    Unreachable(String),
    Failed(String),
    Refused(String),
    FellBack(Box<CallError>),
}

impl CallError {
    fn through_url(self) -> CallError {
        match self {
            CallError::FellBack(e) => *e,
            e => e,
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotConnected(e)
            | CallError::Unreachable(e)
            | CallError::Failed(e)
            | CallError::Refused(e) => f.write_str(e),
            CallError::FellBack(e) => e.fmt(f),
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
    daemon: &Daemon,
    action: &str,
    body: &impl Serialize,
    connect: Option<Duration>,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let token = env_var("LOOM_TOKEN").map_err(CallError::Refused)?;
    post_as(daemon, &token, action, body, connect, timeout)
}

fn post_as<T: DeserializeOwned>(
    daemon: &Daemon,
    token: &str,
    action: &str,
    body: &impl Serialize,
    connect: Option<Duration>,
    timeout: Option<Duration>,
) -> Result<T, CallError> {
    let (agent, url) = daemon.agent(connect, timeout);
    let mut res = agent
        .post(format!("{url}/api/agent/{action}"))
        .header("authorization", format!("Bearer {token}"))
        .header(TOOL_HEADER, tool())
        .header(VERSION_HEADER, version())
        .header(NODE_CALLS_HEADER, NODE_CALLS.now.to_string())
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::ConnectionFailed => CallError::NotConnected(daemon.not_answered(e)),
            e => CallError::Unreachable(daemon.not_answered(e)),
        })?;
    let status = res.status();
    let text = res.body_mut().read_to_string().map_err(|e| {
        CallError::Unreachable(format!(
            "The daemon closed the connection before its answer was complete: {e}."
        ))
    })?;
    if status.is_server_error() {
        return Err(CallError::Failed(format!(
            "The daemon could not answer ({status}): {text}"
        )));
    }
    if !status.is_success() {
        return Err(CallError::Refused(text));
    }
    serde_json::from_str(&text).map_err(|e| CallError::Refused(e.to_string()))
}
