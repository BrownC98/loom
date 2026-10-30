use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use super::server::Api;
use serde_json::{Value, json};

use super::{Permission, TurnEvent, usage};
use crate::profile::adapter::LaunchAt;
use crate::protocol::Model;

pub fn config(at: &LaunchAt<'_>, model: &Model, bypass: &[String]) -> String {
    let mut config = json!({ "agent": { "build": { "prompt": usage(at.launch) } }, "skills": { "paths": [at.skills] } });
    if !bypass.is_empty() {
        config["permission"] = json!("allow");
    }
    if let Some(variant) = &model.effort {
        config["agent"]["build"]["variant"] = json!(variant);
    }
    if let Some(model) = &model.model {
        config["model"] = json!(model);
    }
    config.to_string()
}

pub fn selected_model(event: &Value, session: &str) -> Option<Model> {
    let info = &event["properties"]["info"];
    if event["type"] != "message.updated" || info["sessionID"] != session {
        return None;
    }
    message_model(info)
}

pub fn message_model(info: &Value) -> Option<Model> {
    if info["role"] != "user" {
        return None;
    }
    Some(Model {
        model: Some(format!(
            "{}/{}",
            info["model"]["providerID"].as_str()?,
            info["model"]["modelID"].as_str()?
        )),
        effort: info["variant"]
            .as_str()
            .or_else(|| info["model"]["variant"].as_str())
            .map(str::to_owned),
    })
}

#[derive(Default)]
pub struct Events {
    busy: bool,
    stopped: bool,
    users: HashSet<String>,
}

impl Events {
    pub fn turn(&mut self, event: &Value, session: &str) -> Option<TurnEvent> {
        let data = &event["properties"];
        let event_session = data["sessionID"]
            .as_str()
            .or_else(|| data["info"]["sessionID"].as_str())
            .or_else(|| data["part"]["sessionID"].as_str());
        if event_session != Some(session) {
            return None;
        }
        match event["type"].as_str()? {
            "session.status" if data["status"]["type"] == "busy" && !self.busy => {
                self.busy = true;
                self.stopped = false;
                Some(TurnEvent::Started)
            }
            "message.part.updated"
                if data["part"]["type"] == "text" && data["part"]["time"]["end"].is_number() =>
            {
                Some(TurnEvent::Said(data["part"]["text"].as_str()?.to_owned()))
            }
            "session.error" => {
                self.stopped = true;
                Some(TurnEvent::Stopped)
            }
            "session.idle" | "session.status"
                if event["type"] == "session.idle" || data["status"]["type"] == "idle" =>
            {
                if !std::mem::take(&mut self.busy) {
                    return None;
                }
                Some(if self.stopped {
                    TurnEvent::Stopped
                } else {
                    TurnEvent::Succeeded
                })
            }
            "message.updated" if data["info"]["role"] == "user" => {
                let id = data["info"]["id"].as_str()?;
                if id.contains("_loom_") || !self.users.insert(id.to_owned()) {
                    return None;
                }
                Some(TurnEvent::Enqueued { id: id.to_owned() })
            }
            _ => None,
        }
    }
}

pub fn permission_event(event: &Value, session: &str) -> bool {
    event["properties"]["sessionID"] == session
        && matches!(
            event["type"].as_str(),
            Some("permission.asked" | "permission.replied")
        )
}

pub fn permission(reply: &Value, session: &str) -> Result<Option<Permission>, String> {
    let requests = reply
        .as_array()
        .ok_or_else(|| "permission list: expected array".to_owned())?;
    requests
        .iter()
        .find(|r| r["sessionID"] == session)
        .map(|r| {
            Ok(Permission {
                id: r["id"]
                    .as_str()
                    .ok_or("permission id is absent")?
                    .to_owned(),
                action: r["permission"]
                    .as_str()
                    .ok_or("permission name is absent")?
                    .to_owned(),
                resources: serde_json::from_value(r["patterns"].clone())
                    .map_err(|e| e.to_string())?,
            })
        })
        .transpose()
}

pub fn message_id(api: Api, logical: &str) -> Result<String, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    const COUNTER_BITS: u32 = 12;
    const KILO_TIME_MASK: u64 = 0xffffffffffff;
    let time = (millis << COUNTER_BITS) as u64;
    let prefix = match api {
        Api::MiMo => format!("g{time:016x}"),
        Api::Kilo => format!("{:012x}", time & KILO_TIME_MASK),
        Api::V2 => return Err("v1 message id requested for v2".into()),
    };
    let suffix = logical.strip_prefix("msg_").unwrap_or(logical);
    Ok(format!("msg_{prefix}_{suffix}"))
}
