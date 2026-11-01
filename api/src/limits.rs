use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 노드 나무의 상한. 사람이 설정에서 바꾼다. 근거: docs/design/메시징.md 「상한」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeLimits {
    /// 사람이 띄운 노드가 깊이 0이다. 0이면 에이전트가 자식을 만들지 못한다. `None`이면 상한이 없다.
    pub max_depth: Option<u32>,
    /// 한 부모의 자식 수. `None`이면 상한이 없다.
    pub max_children: Option<u32>,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_depth: Some(2),
            max_children: None,
        }
    }
}
