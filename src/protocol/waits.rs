use serde::{Deserialize, Serialize};

pub use api::graph::waits::{
    InputField, Prompt, PromptKey, PromptOption, Strokes, WaitDoc, WaitKind, entered,
};
pub use api::ids::WaitId;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: WaitId,
    pub text: String,
}
