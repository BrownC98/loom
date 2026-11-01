use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(10);

pub fn set(base: &str, node: u32, on: bool) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(LIMIT))
        .build()
        .into();
    let url = format!("{base}/api/nodes/{node}/bookmark");
    let sent = if on {
        agent.put(&url).send_empty().map(|_| ())
    } else {
        agent.delete(&url).call().map(|_| ())
    };
    sent.map_err(|e| e.to_string())
}
