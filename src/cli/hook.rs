use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::endpoint::Close;
use super::{call, interrupt_arrived, wait_until_mail};
use crate::daemon::Relaunched;
use crate::profile::{
    BATCH_LIMIT_BYTES, Handed, Hook, HookEntry, HookEvent, LineEvent, Served, Shown, Signals, Take,
    Unserved,
};
use crate::protocol::{
    AckRequest, EndedTurn, IdleReply, IdleRequest, Kind, MessageId, TakeRequest, Taken,
};
use crate::wsl::REWAKE_EXIT;

const CALL_LIMIT: Duration = Duration::from_secs(5);

pub fn hook(entry: HookEntry) -> Result<(), String> {
    match crate::profile::hook(entry) {
        Hook::Once(parse) => {
            let event = serde_json::from_reader(std::io::stdin())
                .and_then(parse)
                .map_err(|e| format!("hook input: {e}"))?;
            once(&*event)
        }
        Hook::Lines(parse) => lines(parse),
    }
}

fn once(event: &dyn HookEvent) -> Result<(), String> {
    let (rewake, unacked) = run(event, print)?;
    unacked.acknowledge();
    if rewake {
        std::process::exit(REWAKE_EXIT)
    }
    Ok(())
}

fn lines(parse: fn(&str) -> Result<Box<dyn LineEvent>, Unserved>) -> Result<(), String> {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let waits: Arc<Mutex<HashMap<u64, Close>>> = Arc::default();
    for line in std::io::stdin().lines() {
        let line = line.map_err(|e| format!("hook input: {e}"))?;
        let request = match parse(&line) {
            Ok(request) => request,
            Err(refused) => return unserved(refused, &out),
        };
        let out = out.clone();
        let send = move |line: String| send_line(&out, line);
        let Some(id) = request.id() else {
            return run(&*request, |shown| send(shown.out.unwrap_or_default()))
                .map(|(_, unacked)| unacked.acknowledge());
        };
        let close = Close::default();
        if let Served::Interrupt = request.served() {
            waits.lock().unwrap().insert(id, close.clone());
        }
        let waits = waits.clone();
        std::thread::spawn(move || {
            let served = match request.served() {
                Served::Ack(ids) => call::<()>("ack", &AckRequest { ids: ids.to_vec() }, None)
                    .map_err(String::from)
                    .and_then(|()| send(request.acked())),
                Served::Close(of) => {
                    if let Some(close) = waits.lock().unwrap().remove(&of) {
                        close.close();
                    }
                    send(request.acked())
                }
                Served::Interrupt => {
                    let arrived = interrupt_arrived(close.clone());
                    waits.lock().unwrap().remove(&id);
                    match arrived {
                        Ok(true) => send(request.interrupted()),
                        Ok(false) => send(request.acked()),
                        Err(_) if close.closed() => send(request.acked()),
                        Err(e) => Err(e),
                    }
                }
                Served::Hook => {
                    run(&*request, |shown| send(shown.out.unwrap_or_default())).map(drop)
                }
            };
            if let Err(e) = served {
                let _ = send(request.failed(e));
            }
        });
    }
    Ok(())
}

fn unserved(unserved: Unserved, out: &Mutex<std::io::Stdout>) -> Result<(), String> {
    if let Some(reply) = unserved.reply {
        send_line(out, reply)?;
    }
    let relaunch = match call::<Relaunched>("relaunch-unserved", &(), Some(CALL_LIMIT)) {
        Ok(_) => format!(
            "loom relaunches this node with the current {}",
            unserved.extension
        ),
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
    Listen(IdleRequest),
    Woken(TakeRequest),
}

fn step(signals: &Signals) -> Step {
    match signals.take {
        None => Step::Skip,
        Some(Take::WhenIdle) => Step::Idle(idle_request(signals)),
        Some(Take::Listen) => Step::Listen(idle_request(signals)),
        Some(Take::Now(kinds)) => Step::Take(take_request(signals, kinds)),
        Some(Take::Woken) => Step::Woken(take_request(signals, Kind::ALL)),
    }
}

fn idle_request(signals: &Signals) -> IdleRequest {
    IdleRequest {
        session: signals.session.clone(),
        rate_limited: signals.rate_limited,
        turn_end: signals.turn_end.clone().map(|reply| EndedTurn {
            reply,
            turn: signals.turn.clone().or_else(ended_turn),
        }),
    }
}

fn take_request(signals: &Signals, kinds: &[Kind]) -> TakeRequest {
    TakeRequest {
        step: signals.step.clone(),
        kinds: kinds.to_vec(),
        limit_bytes: BATCH_LIMIT_BYTES,
        session: signals.session.clone(),
        human_input: signals.human_input,
        prompt: signals.prompt.clone(),
        turn: signals.turn.clone(),
    }
}

fn handed(reply: IdleReply) -> Handed {
    match reply {
        IdleReply::Nudge { nudge } => Handed::Nudge(nudge),
        IdleReply::Mail(messages) => Handed::Mail(messages),
    }
}

fn ended_turn() -> Option<String> {
    getrandom::u64().ok().map(|n| format!("loom-{n:016x}"))
}

struct Unacked {
    ids: Vec<MessageId>,
    limit: Option<Duration>,
}

impl Unacked {
    fn acknowledge(self) {
        if self.ids.is_empty() {
            return;
        }
        if let Err(e) = call::<()>("ack", &AckRequest { ids: self.ids }, self.limit) {
            eprintln!("loom: could not acknowledge: {e}");
        }
    }
}

fn run(
    event: &dyn HookEvent,
    write: impl FnOnce(Shown) -> Result<(), String>,
) -> Result<(bool, Unacked), String> {
    let mut signals = event.signals();
    for report in std::mem::take(&mut signals.reports) {
        if let Err(e) = call::<()>("report", &report, Some(CALL_LIMIT)) {
            eprintln!("loom: could not report to loom: {e}");
        }
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
        Step::Idle(request) => (handed(wait_until_mail(request)?), None),
        Step::Listen(request) => {
            let reply = call("listen", &request, Some(CALL_LIMIT)).unwrap_or_else(|e| {
                eprintln!("loom: could not report the idle state to loom: {e}");
                IdleReply::Mail(Vec::new())
            });
            (handed(reply), Some(CALL_LIMIT))
        }
        Step::Woken(request) => (
            handed(call("wake", &request, Some(CALL_LIMIT))?),
            Some(CALL_LIMIT),
        ),
        Step::Take(request) => {
            let Taken { messages, note } = call("take", &request, Some(CALL_LIMIT))?;
            (Handed::Taken { messages, note }, Some(CALL_LIMIT))
        }
    };
    let shown = event.show(&handed);
    let rewake = shown.rewake;
    write(shown)?;
    let unacked = Unacked {
        ids: handed.messages().iter().map(|d| d.id).collect(),
        limit: ack_limit,
    };
    Ok((rewake, unacked))
}

fn interrupt_pending() -> bool {
    call("interrupt-pending", &(), Some(CALL_LIMIT)).unwrap_or_else(|e| {
        eprintln!("loom: could not check for an interrupt: {e}");
        false
    })
}
