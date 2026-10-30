use serde::{Deserialize, Serialize};

use super::{Handed, HookEvent, LineEvent, Served, Shown, Signals, Take, Unserved};
use crate::protocol::{
    ChoiceRequest, Delivery, Kind, MessageId, Model, Prompt, PromptKey, PromptOption, Report,
    Session, WaitKind,
};

const UNNUMBERED: u32 = 2;
const PROTOCOL: u32 = 4;

struct Request {
    id: Option<u64>,
    event: Event,
}

#[derive(Deserialize)]
struct Header {
    protocol: Option<u32>,
    id: Option<u64>,
}

pub(super) fn request(line: &str, extension: &'static str) -> Result<Box<dyn LineEvent>, Unserved> {
    let header: Header = serde_json::from_str(line).map_err(|e| Unserved {
        reply: None,
        error: format!("hook input: {e}"),
        extension,
    })?;
    let id = header.id;
    let unserved = |error: String| {
        Err(Unserved {
            reply: id.map(|_| failed(id, error.clone()).line()),
            error,
            extension,
        })
    };
    match (header.protocol, id) {
        (Some(PROTOCOL), Some(_)) | (None, None) => {}
        (Some(PROTOCOL), None) => {
            return unserved(format!("a hook protocol {PROTOCOL} request has no id"));
        }
        (other, _) => {
            let other = other.unwrap_or(UNNUMBERED);
            return unserved(format!(
                "the {extension} speaks hook protocol {other}, but this loom speaks {PROTOCOL}"
            ));
        }
    }
    match serde_json::from_str(line) {
        Ok(event) => Ok(Box::new(Request { id, event })),
        Err(e) => unserved(format!("hook input: {e}")),
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    SessionStart {
        #[serde(flatten)]
        state: State,
    },
    Idle {
        #[serde(flatten)]
        state: State,
    },
    Prompt {
        prompt: String,
        #[serde(flatten)]
        state: State,
    },
    InTurn {
        #[serde(flatten)]
        state: State,
    },
    ToolCall {
        #[serde(flatten)]
        state: State,
    },
    ToolResult {
        #[serde(flatten)]
        state: State,
    },
    Settled {
        reply: Option<String>,
        #[serde(flatten)]
        state: State,
    },
    Ack {
        ids: Vec<MessageId>,
        #[serde(flatten)]
        state: State,
    },
    Interrupt {
        #[serde(flatten)]
        state: State,
    },
    InterruptPending {
        #[serde(flatten)]
        state: State,
    },
    Approval {
        summary: String,
        body: Vec<String>,
        options: Vec<Choice>,
        #[serde(flatten)]
        state: State,
    },
    Close {
        of: u64,
        #[serde(flatten)]
        state: State,
    },
}

#[derive(Deserialize)]
struct Choice {
    label: String,
    reply: String,
}

#[derive(Deserialize)]
struct State {
    transcript: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ids: Vec<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    interrupted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    choice: Option<String>,
}

impl Event {
    fn state(&self) -> &State {
        match self {
            Event::SessionStart { state }
            | Event::Idle { state }
            | Event::Prompt { state, .. }
            | Event::InTurn { state }
            | Event::ToolCall { state }
            | Event::ToolResult { state }
            | Event::Settled { state, .. }
            | Event::Ack { state, .. }
            | Event::Interrupt { state }
            | Event::InterruptPending { state }
            | Event::Approval { state, .. }
            | Event::Close { state, .. } => state,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Event::SessionStart { .. } => "session_start",
            Event::Idle { .. } => "idle",
            Event::Prompt { .. } => "prompt",
            Event::InTurn { .. } => "in_turn",
            Event::ToolCall { .. } => "tool_call",
            Event::ToolResult { .. } => "tool_result",
            Event::Settled { .. } => "settled",
            Event::Ack { .. } => "ack",
            Event::Interrupt { .. } => "interrupt",
            Event::InterruptPending { .. } => "interrupt_pending",
            Event::Approval { .. } => "approval",
            Event::Close { .. } => "close",
        }
    }

    fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. }
            | Event::Ack { .. }
            | Event::Interrupt { .. }
            | Event::InterruptPending { .. }
            | Event::Approval { .. }
            | Event::Close { .. } => return None,
            Event::Idle { .. } | Event::Settled { .. } => Take::WhenIdle,
            Event::Prompt { .. } | Event::ToolResult { .. } => Take::Now(Kind::ALL),
            Event::InTurn { .. } => Take::Now(&[Kind::Queue, Kind::Steer]),
            Event::ToolCall { .. } => Take::Now(&[Kind::Interrupt]),
        })
    }

    fn transcript(&self) -> Option<String> {
        self.state().transcript.clone()
    }

    fn model(&self) -> Option<Model> {
        match self {
            Event::SessionStart { state }
            | Event::Prompt { state, .. }
            | Event::Settled { state, .. }
                if state.model.is_some() =>
            {
                Some(Model {
                    model: state.model.clone(),
                    effort: state.effort.clone(),
                })
            }
            _ => None,
        }
    }

    fn prompt(&self) -> Option<&str> {
        match self {
            Event::Prompt { prompt, .. } => Some(prompt),
            _ => None,
        }
    }

    fn turn_ends(&self) -> Option<Option<String>> {
        match self {
            Event::Settled { reply, .. } => Some(reply.clone()),
            _ => None,
        }
    }
}

impl Reply {
    fn line(&self) -> String {
        serde_json::to_string(self).expect("답은 늘 직렬화된다")
    }
}

impl LineEvent for Request {
    fn id(&self) -> Option<u64> {
        self.id
    }

    fn served(&self) -> Served<'_> {
        match &self.event {
            Event::Ack { ids, .. } => Served::Ack(ids),
            Event::Interrupt { .. } => Served::Interrupt,
            Event::Approval {
                summary,
                body,
                options,
                ..
            } => Served::Choice(ChoiceRequest {
                kind: WaitKind::Approval,
                prompt: Prompt {
                    rows: std::iter::once(summary.clone())
                        .chain(body.iter().cloned())
                        .collect(),
                    body: body.clone(),
                    summary: summary.clone(),
                    options: options
                        .iter()
                        .map(|o| PromptOption {
                            label: o.label.clone(),
                            key: PromptKey::Reply(o.reply.clone()),
                            writes: false,
                        })
                        .collect(),
                    field: None,
                },
            }),
            Event::Close { of, .. } => Served::Close(*of),
            _ => Served::Hook,
        }
    }

    fn acked(&self) -> String {
        acked(self.id).line()
    }

    fn interrupted(&self) -> String {
        Reply {
            interrupted: true,
            ..acked(self.id)
        }
        .line()
    }

    fn chosen(&self, reply: &str) -> String {
        Reply {
            choice: Some(reply.to_owned()),
            ..acked(self.id)
        }
        .line()
    }

    fn failed(&self, error: String) -> String {
        failed(self.id, error).line()
    }
}

impl HookEvent for Request {
    fn signals(&self) -> Signals {
        let event = &self.event;
        Signals {
            step: event.name().to_owned(),
            take: event.take(),
            session: event.transcript().map(Session),
            prompt: event.prompt().map(str::to_owned),
            human_input: event.prompt().is_some(),
            turn_end: event.turn_ends(),
            reports: event.model().map(Report::Model).into_iter().collect(),
            asks_interrupt: matches!(event, Event::InterruptPending { .. }),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let reply = match handed {
            Handed::Taken { messages, note } => reply(self.id, messages, note.as_deref()),
            Handed::Mail(messages) => reply(self.id, messages, None),
            Handed::Nudge(text) => nudge(self.id, text.clone()),
            Handed::Interrupted => Reply {
                interrupted: true,
                ..acked(self.id)
            },
        };
        Shown {
            out: Some(reply.line()),
            ..Shown::default()
        }
    }
}

fn reply(id: Option<u64>, taken: &[Delivery], note: Option<&str>) -> Reply {
    Reply {
        id,
        text: super::hook_text(taken, note),
        ids: taken.iter().map(|d| d.id).collect(),
        error: None,
        interrupted: false,
        choice: None,
    }
}

fn nudge(id: Option<u64>, text: String) -> Reply {
    Reply {
        id,
        text: Some(text),
        ids: Vec::new(),
        error: None,
        interrupted: false,
        choice: None,
    }
}

fn acked(id: Option<u64>) -> Reply {
    Reply {
        id,
        text: None,
        ids: Vec::new(),
        error: None,
        interrupted: false,
        choice: None,
    }
}

fn failed(id: Option<u64>, error: String) -> Reply {
    Reply {
        id,
        text: None,
        ids: Vec::new(),
        error: Some(error),
        interrupted: false,
        choice: None,
    }
}
