use std::time::Duration;

use super::pane::Pane;

const LIMIT: Duration = Duration::from_secs(10);

pub fn pane_width(base: &str, pane: Pane, width: u16) -> Result<(), String> {
    let patch = serde_json::json!({ "paneWidths": { pane.key(): width } });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(LIMIT))
        .build()
        .into();
    agent
        .put(format!("{base}/api/settings"))
        .header("content-type", "application/json")
        .send(patch.to_string())
        .map(|_| ())
        .map_err(|e| e.to_string())
}
