use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// WSL 배포판 하나의 홈과 기본 워크트리 폴더. 둘 다 UNC 경로다.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DistroView {
    /// 위치 칩을 누르면 폴더 목록이 가는 곳.
    pub home: String,
    /// 설정에 이 배포판의 폴더가 없을 때 쓰는 폴더(`<홈>/loom/workspaces`).
    pub workspace: String,
}

/// SSH 호스트에 닿았는지. `reached`가 아니면 닿지 않은 까닭이다.
#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ReachView {
    Reached { home: String, system: SystemView },
    UnknownHostKey { folder: String },
    AuthFailed { folder: String },
    Unreachable { cause: String },
    UnsupportedMac { cpu: String },
    VersionMismatch,
    Failed { cause: String },
}

#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SystemView {
    Linux,
    Mac,
    Windows { distro: String },
}
