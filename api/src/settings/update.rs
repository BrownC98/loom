use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 화면이 보는 업데이트 상태. 받는 동안의 퍼센트를 포함해 바뀔 때마다 `ViewChanged`를 보낸다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateView {
    /// 이 데몬이 확인하고 바꿀 수 있다. 개발 빌드와 받을 파일이 없는 OS는 거짓이다.
    pub checks: bool,
    /// 이 데몬보다 새 판. 없으면 최신이다(또는 아직 확인하지 않았다).
    pub latest: Option<String>,
    pub step: Option<UpdateStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateStep {
    Available,
    Downloading { percent: u8 },
    Restarting,
    Failed { message: String },
}
