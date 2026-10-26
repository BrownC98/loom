use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use super::{call, wait_until_mail};
use crate::daemon::{
    AckRequest, EndedTurn, HookWaitRequest, IdleReply, IdleRequest, Relaunched, SaidRequest,
    TakeRequest, Taken,
};
use crate::profile::{
    BATCH_LIMIT_BYTES, Handed, HookEvent, Report, Shown, Signals, Take, claude, cursor, pi,
};
use crate::protocol::Delivery;
use crate::wsl::REWAKE_EXIT;

const CALL_LIMIT: Duration = Duration::from_secs(5);

pub fn claude_hook() -> Result<(), String> {
    let hook = claude::Hook::deserialize(read_input()?).map_err(|e| format!("hook input: {e}"))?;
    if run(&hook, print)? {
        std::process::exit(REWAKE_EXIT)
    }
    Ok(())
}

pub fn cursor_hook() -> Result<(), String> {
    let hook = cursor::Hook::parse(read_input()?).map_err(|e| format!("hook input: {e}"))?;
    run(&hook, print).map(drop)
}

pub fn pi_hook() -> Result<(), String> {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    for line in std::io::stdin().lines() {
        let line = line.map_err(|e| format!("hook input: {e}"))?;
        let request = match pi::request(&line) {
            Ok(request) => request,
            Err(unserved) => return pi_unserved(unserved, &out),
        };
        let out = out.clone();
        let send = move |line: String| send_line(&out, line);
        let Some(id) = request.id else {
            return run(&request, |shown| send(shown.out.unwrap_or_default())).map(drop);
        };
        std::thread::spawn(move || {
            if let Err(e) = run(&request, |shown| send(shown.out.unwrap_or_default())) {
                let _ = send(pi::failed(Some(id), e).line());
            }
        });
    }
    Ok(())
}

fn read_input() -> Result<serde_json::Value, String> {
    serde_json::from_reader(std::io::stdin()).map_err(|e| format!("hook input: {e}"))
}

fn pi_unserved(unserved: pi::Unserved, out: &Mutex<std::io::Stdout>) -> Result<(), String> {
    if unserved.id.is_some() {
        send_line(out, pi::failed(unserved.id, unserved.error.clone()).line())?;
    }
    let relaunch = match call::<Relaunched>("relaunch-unserved", &(), Some(CALL_LIMIT)) {
        Ok(_) => "loom relaunches this node with the current pi extension".to_owned(),
        Err(e) => format!("loom could not relaunch this node: {e}"),
    };
    Err(format!("{}. {relaunch}.", unserved.error))
}

fn send_line(out: &Mutex<std::io::Stdout>, line: String) -> Result<(), String> {
    let mut out = out.lock().unwrap();
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

fn print(shown: Shown) -> Result<(), String> {
    if let Some(out) = shown.out {
        let mut stdout = std::io::stdout();
        writeln!(stdout, "{out}")
            .and_then(|()| stdout.flush())
            .map_err(|e| e.to_string())?;
    }
    if let Some(text) = shown.to_model {
        let mut stderr = std::io::stderr();
        writeln!(stderr, "{text}")
            .and_then(|()| stderr.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

enum Step {
    Skip,
    Take(TakeRequest),
    Idle(IdleRequest),
}

fn step(signals: &Signals) -> Step {
    match signals.take {
        None => Step::Skip,
        Some(Take::WhenIdle) => Step::Idle(IdleRequest {
            transcript: signals.transcript.clone(),
            rate_limited: signals.rate_limited,
            turn_end: signals.turn_end.clone().map(|reply| EndedTurn {
                reply,
                turn: signals.turn.clone(),
            }),
        }),
        Some(Take::Now(kinds)) => Step::Take(TakeRequest {
            hook: signals.hook.clone(),
            kinds: kinds.to_vec(),
            limit_bytes: BATCH_LIMIT_BYTES,
            transcript: signals.transcript.clone(),
            human_input: signals.human_input,
            prompt: signals.prompt.clone(),
            turn: signals.turn.clone(),
        }),
    }
}

fn run(
    event: &impl HookEvent,
    write: impl FnOnce(Shown) -> Result<(), String>,
) -> Result<bool, String> {
    let mut signals = event.signals();
    if let Some(model) = &signals.model
        && let Err(e) = call::<()>("model", model, Some(CALL_LIMIT))
    {
        eprintln!("loom: could not report the model: {e}");
    }
    for report in std::mem::take(&mut signals.reports) {
        send_report(report, signals.turn.clone());
    }
    let (handed, ack_limit) = match step(&signals) {
        Step::Skip => {
            let handed = match signals.asks_interrupt && interrupt_pending() {
                true => Handed::Interrupted,
                false => Handed::Taken {
                    messages: Vec::new(),
                    note: None,
                },
            };
            (handed, Some(CALL_LIMIT))
        }
        Step::Idle(request) => {
            let handed = match wait_until_mail(request)? {
                IdleReply::Nudge { nudge } => Handed::Nudge(nudge),
                IdleReply::Mail(messages) => Handed::Mail(messages),
            };
            (handed, None)
        }
        Step::Take(request) => {
            let Taken { messages, note } = call("take", &request, Some(CALL_LIMIT))?;
            (Handed::Taken { messages, note }, Some(CALL_LIMIT))
        }
    };
    let shown = event.show(&handed);
    let rewake = shown.rewake;
    write(shown)?;
    acknowledge(handed.messages(), ack_limit);
    Ok(rewake)
}

fn send_report(report: Report, turn: Option<String>) {
    let (what, sent) = match report {
        Report::Wait(wait) => (
            "the human wait",
            call::<()>("hook-wait", &HookWaitRequest { wait }, Some(CALL_LIMIT)),
        ),
        Report::Said(text) => (
            "the reply",
            call::<()>("said", &SaidRequest { text, turn }, Some(CALL_LIMIT)),
        ),
        Report::QuestionBox => (
            "the question box",
            call::<()>("question-box", &(), Some(CALL_LIMIT)),
        ),
    };
    if let Err(e) = sent {
        eprintln!("loom: could not report {what}: {e}");
    }
}

fn interrupt_pending() -> bool {
    call("interrupt-pending", &(), Some(CALL_LIMIT)).unwrap_or_else(|e| {
        eprintln!("loom: could not check for an interrupt: {e}");
        false
    })
}

fn acknowledge(taken: &[Delivery], limit: Option<Duration>) {
    if taken.is_empty() {
        return;
    }
    let ids = taken.iter().map(|d| d.id).collect();
    if let Err(e) = call::<()>("ack", &AckRequest { ids }, limit) {
        eprintln!("loom: could not acknowledge: {e}");
    }
}
