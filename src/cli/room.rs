use std::time::{Duration, Instant};

use api::graph::docs::{PostOutside, RoomTalk, Said, TalkLine};
use api::graph::errors::Problem;
use api::graph::{NodeView, ProcessState};
use serde::Serialize;
use serde::de::DeserializeOwned;

const CALL_FOR: Duration = Duration::from_secs(10);
const POLL_EVERY: Duration = Duration::from_secs(1);

pub fn nodes() -> Result<(), String> {
    let nodes: Vec<NodeView> = get(&daemon()?, "/api/nodes")?;
    for node in nodes
        .iter()
        .filter(|n| n.agent.is_some() && !matches!(n.process, ProcessState::Stopped { .. }))
    {
        println!("#{} {} ({})", node.id.0, node.name, node.place.worktree);
    }
    Ok(())
}

pub fn send(node: u32, from: String, text: String) -> Result<(), String> {
    let line: u32 = post(
        &daemon()?,
        &format!("/api/rooms/{node}/outside"),
        &PostOutside { from, text },
    )?;
    println!(
        "Sent to #{node} as line #{line}. Read the reply with: loom room read {node} --after {line} --wait 60"
    );
    Ok(())
}

pub fn read(node: u32, after: u32, wait: u64) -> Result<(), String> {
    let url = daemon()?;
    let until = Instant::now() + Duration::from_secs(wait);
    loop {
        let talk: RoomTalk = get(&url, &format!("/api/rooms/{node}/talk"))?;
        let new: Vec<&TalkLine> = talk.lines.iter().filter(|l| l.id > after).collect();
        let replied = new.iter().any(|l| answers_outside(&talk.lines, &l.said));
        if replied || Instant::now() >= until {
            for line in &new {
                println!("#{} {}: {}", line.id, who(&line.said), line.text);
            }
            if !replied {
                println!("No reply yet.");
            }
            return Ok(());
        }
        std::thread::sleep(POLL_EVERY);
    }
}

fn answers_outside(lines: &[TalkLine], said: &Said) -> bool {
    let Said::Agent {
        reply_to: Some(asked),
        ..
    } = said
    else {
        return false;
    };
    lines
        .iter()
        .any(|l| l.id == *asked && matches!(l.said, Said::Outside { .. }))
}

fn who(said: &Said) -> String {
    match said {
        Said::Agent { name, .. } => name.clone(),
        Said::Human { to, .. } => format!("human -> {to}"),
        Said::Outside { from, to, .. } => format!("{from} -> {to}"),
    }
}

fn daemon() -> Result<String, String> {
    crate::launch::daemon_url().ok_or_else(|| "no loom daemon runs here (no daemon.json)".into())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(CALL_FOR))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

fn get<T: DeserializeOwned>(url: &str, path: &str) -> Result<T, String> {
    let response = agent().get(format!("{url}{path}")).call();
    answer(response)
}

fn post<T: DeserializeOwned>(url: &str, path: &str, body: &impl Serialize) -> Result<T, String> {
    let response = agent().post(format!("{url}{path}")).send_json(body);
    answer(response)
}

fn answer<T: DeserializeOwned>(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<T, String> {
    let mut response = response.map_err(|e| format!("could not reach the loom daemon: {e}"))?;
    let ok = response.status().is_success();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if !ok {
        return Err(match serde_json::from_str::<Problem>(&body) {
            Ok(Problem::Refused(refusal)) => refusal.to_string(),
            _ => body,
        });
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}
