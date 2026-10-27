use serde::{Deserialize, Serialize};

use super::{Delivery, Kind, MessageId, Model, Prompt, WaitKind};

#[derive(Clone, Serialize, Deserialize)]
pub struct Session(pub String);

#[derive(Clone, Serialize, Deserialize)]
pub struct AdapterRecord(pub serde_json::Value);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Report {
    Model(Model),
    Session(Session),
    HumanWait(Option<(WaitKind, String, Option<Prompt>)>),
    Said { text: String, turn: Option<String> },
    QuestionBox,
    RateLimited,
    TurnStopped { turn: Option<String> },
    Busy,
    Lost,
    Refused { ids: Vec<MessageId>, cause: String },
}

#[derive(Serialize, Deserialize)]
pub struct TakeRequest {
    #[serde(default, alias = "hook")]
    pub step: String,
    pub kinds: Vec<Kind>,
    pub limit_bytes: usize,
    #[serde(default, rename = "transcript")]
    pub session: Option<Session>,
    #[serde(default)]
    pub human_input: bool,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub turn: Option<String>,
    #[serde(default)]
    pub prompt_id: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Taken {
    pub messages: Vec<Delivery>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleRequest {
    #[serde(rename = "transcript")]
    pub session: Option<Session>,
    pub rate_limited: bool,
    pub turn_end: Option<EndedTurn>,
    pub prompt_id: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EndedTurn {
    pub reply: Option<String>,
    #[serde(default)]
    pub turn: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum IdleReply {
    Mail(Vec<Delivery>),
    Nudge { nudge: String },
}

#[derive(Serialize, Deserialize)]
pub struct AckRequest {
    pub ids: Vec<MessageId>,
}
