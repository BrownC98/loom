use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use api::graph::errors::{InsideError, LaunchError};
pub use api::ids::AgentId;
pub use api::profile::Provider;
pub use api::start::ModelChoice;

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Model {
    pub model: Option<String>,
    pub effort: Option<String>,
}

pub type StartDigests = BTreeMap<String, String>;
