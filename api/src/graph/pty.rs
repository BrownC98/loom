use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// PTY 크기. 데몬은 바뀐 크기를 모든 화면에 알린다(`PtyNotice::Size`).
#[derive(Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
}

/// 화면이 PTY 웹소켓에 보내는 글 프레임. 키 입력은 바이너리 프레임으로 간다(docs/design/데몬-하나.md 「데몬 하나」 PTY 열·줄).
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PtyRequest {
    /// 이 크기를 쥔다. PC 화면이 보낸다.
    Resize(Resize),
    /// 이 연결이 보는 동안만 이 크기를 쓴다. 폰이 보낸다. 연결이 닫히거나 `GiveBack`이 오면 빌리기 전 크기로 돌린다.
    Borrow(Resize),
    /// 빌린 크기를 돌려준다. 폰 화면이 가려질 때 보낸다.
    GiveBack,
}

/// PTY 웹소켓의 글 프레임. 터미널 출력은 바이너리 프레임으로 간다.
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PtyNotice {
    /// 이 PTY가 없다. 데몬이 다시 시작할 때 호스트가 사라졌거나, 앞 데몬에서 끝난 노드다.
    Gone,
    /// PTY의 열·줄. 붙을 때 backlog보다 먼저, 그 뒤 바뀔 때마다 보낸다. 화면은 이 크기로 그리고 칸에 맞게 늘이거나 줄인다.
    Size(Resize),
    /// 붙을 때 PTY가 한동안 조용했다(`node_host::Attached::settled`). backlog보다 먼저 보낸다. 화면은 그 backlog 속 색 질의(OSC 10·11·4)에 답하지 않는다.
    /// 근거: 카드를 열 때마다 xterm이 옛 질의에 다시 답해 프로그램에 늦은 입력이 갔다(실측 2026-10-23). 갓 보낸 질의는 프로그램이 아직 기다릴 수 있어 답한다.
    Settled,
    /// 연결이 살아 있음을 알린다. 붙은 바로 뒤, `Pace`를 받은 바로 뒤, 그 뒤 박동 간격마다 보낸다. 화면은 `quietMs` 동안 아무 프레임도 받지 못하면 이 연결만 다시 연다.
    Beat {
        #[serde(rename = "quietMs")]
        quiet_ms: u32,
    },
}

/// 화면이 웹소켓에 보내는 글 프레임. 데몬은 이 값에 맞춰 박동 간격을 바꾸고 바로 박동을 보낸다.
/// 보내지 않은 화면에는 15초마다 박동을 보낸다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "pace", rename_all = "camelCase")]
#[ts(export)]
pub enum Pace {
    /// 사람이 화면을 보고 있다.
    Front,
    /// 화면이 뒤에 있다. 휴대폰 서비스는 늘 이것을 보낸다.
    Back,
}
