//! 계정 전환: 실행 중인 노드를 새 계정으로 다시 띄워 같은 세션을 잇고, 켜져 있으면 한도에 닿은 계정을 자동으로 바꾼다.
//! 근거: docs/design.md 「계정 전환」, 목업 docs/mockups/account-switch.html.

use serde::Serialize;
use ts_rs::TS;

use crate::graph::NodeId;

/// 계정을 바꿀 때 옮길 수 있는 실행 중인 노드(`GET /api/accounts/{provider}/nodes`). 확인 창 목록이다(목업 ①).
/// 지금 계정이 설정의 계정과 같은 노드는 넣지 않는다.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MovableNode {
    pub id: NodeId,
    pub name: String,
    /// 턴 중이다. 그 턴이 끝난 뒤 옮긴다.
    pub busy: bool,
}

/// `POST /api/accounts/{provider}/move`의 결과. 설정에 저장한 계정으로 옮긴다. 같은 값을 이벤트 기록에도 남긴다.
#[derive(Clone, Copy, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MoveResult {
    pub moved: u32,
    pub later: u32,
    pub failed: u32,
}
