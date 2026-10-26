use serde::{Deserialize, Serialize};

use super::{Model, Prompt, WaitKind};

#[derive(Clone, Serialize, Deserialize)]
pub struct Session(pub String);

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerState(pub serde_json::Value);

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerRecord {
    pub pid: Option<u32>,
    pub state: ServerState,
}

pub enum Report {
    Model(Model),
    Session(Session),
    HumanWait(Option<(WaitKind, String, Option<Prompt>)>),
    RateLimited,
}
