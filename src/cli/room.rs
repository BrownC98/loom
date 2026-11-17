use std::time::{Duration, Instant};

use api::graph::docs::{PostOutside, RoomTalk, Said, TalkLine};
use api::graph::errors::Problem;
use api::graph::{NodeView, ProcessState};
use serde::Serialize;
use serde::de::DeserializeOwned;

const CALL_FOR: Duration = Duration::from_secs(10);
const POLL_EVERY: Duration = Duration::from_secs(1);

#[allow(clippy::disallowed_macros)]
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

#[allow(clippy::disallowed_macros)]
pub fn send(node: u32, from: String, text: String) -> Result<(), String> {
    let url = daemon()?;
    let talk: RoomTalk = get(&url, &format!("/api/rooms/{node}/talk"))?;
    post::<()>(
        &url,
        &format!("/api/rooms/{node}/outside"),
        &PostOutside { from, text },
    )?;
    let after = talk.lines.last().map_or(0, |line| line.id);
    println!(
        "Sent to #{node}. Read the reply with: loom room read {node} --after {after} --wait 60"
    );
    Ok(())
}

#[allow(clippy::disallowed_macros)]
pub fn read(node: u32, after: u32, wait: u64) -> Result<(), String> {
    let url = daemon()?;
    let until = Instant::now() + Duration::from_secs(wait);
    loop {
        let talk: RoomTalk = get(&url, &format!("/api/rooms/{node}/talk"))?;
        let new: Vec<&TalkLine> = talk.lines.iter().filter(|l| l.id > after).collect();
        let replied = new.iter().any(|l| matches!(l.said, Said::Agent { .. }));
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

fn who(said: &Said) -> String {
    match said {
        Said::Agent { name } => name.clone(),
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
