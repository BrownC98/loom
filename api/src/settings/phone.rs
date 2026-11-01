use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhoneView {
    /// 폰 문이 QR에 담을 주소. 같은 네트워크(사설 주소)가 먼저, Tailscale이 뒤다.
    pub addresses: Vec<PhoneAddress>,
    /// 연 폰 문의 포트. [휴대폰 연결]을 누르기 전이면 없다.
    pub port: Option<u16>,
    /// [방화벽 허용]을 보일지. Windows 데몬만 그렇다.
    pub firewall: bool,
    /// 브라우저(iPhone)가 쓰는 Tailscale Serve 길의 상태.
    pub tailnet: Tailnet,
    pub devices: Vec<DeviceView>,
    /// 화면이 보인 코드를 아직 쓸 수 있는지. 폰이 쓰거나 다섯 번 틀리거나 시간이 지나면 `false`다. 앞의 둘은 `ViewChanged`로 알린다.
    pub invite_open: bool,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhoneAddress {
    pub ip: String,
    /// Tailscale 주소(100.64.0.0/10)다. 폰도 Tailscale에 있으면 밖에서도 닿는다.
    pub tailnet: bool,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InviteView {
    /// QR에 담는 주소. 코드는 `#` 뒤에 있어 브라우저가 서버에 보내지 않는다.
    pub url: String,
    pub code: String,
    /// `url`의 QR 그림(SVG 글).
    pub qr_svg: String,
    /// 코드를 쓸 수 있는 마지막 때(Unix 밀리초). 화면은 남은 시간을 보이기만 하고, 쓸 수 있는지는 데몬이 쓸 때 본다.
    #[ts(type = "number")]
    pub until: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceId(#[ts(type = "number")] pub i64);

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceView {
    pub id: DeviceId,
    pub label: String,
    /// Unix 밀리초.
    #[ts(type = "number")]
    pub paired_at: i64,
    #[ts(type = "number")]
    pub last_seen: i64,
}

/// [Serve 켜기]의 결과.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Served {
    On,
    /// tailnet 관리자가 Serve를 허용해야 한다. `url`은 CLI가 찍은 허용 페이지다. 허용한 뒤 다시 누른다.
    NeedsConsent {
        url: String,
    },
}

/// 이 컴퓨터의 Tailscale 상태.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Tailnet {
    /// CLI를 찾지 못했다.
    Missing,
    /// CLI는 있지만 tailnet에 붙지 않았다(로그아웃, 꺼짐).
    Stopped,
    /// `host`는 이 기기의 HTTPS 이름(`<기기>.<tailnet>.ts.net`)이다. `serving`은 Serve가 데몬 포트로 넘기는지다.
    Ready { host: String, serving: bool },
}
