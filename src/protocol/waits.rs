use serde::{Deserialize, Serialize};

use api::ids::WaitId;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: WaitId,
    pub text: String,
}
