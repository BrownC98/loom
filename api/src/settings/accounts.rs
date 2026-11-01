use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::AccountId;
use crate::profile::Provider;
use crate::settings::usage::Usage;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginId(pub u32);

impl std::fmt::Display for LoginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LoginState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountView {
    pub provider: Provider,
    /// 계정이 있는 WSL 배포판. 이 PC면 없다. 계정은 한 위치에만 속한다.
    pub distro: Option<String>,
    /// 없으면 그 위치의 시스템 기본(그 CLI가 `HOME` 아래에서 고르는 로그인 폴더)이다. 쓰는 계정은 설정(`SettingsView`)이 위치마다 정한다.
    pub id: Option<AccountId>,
    pub usage: Usage,
}

/// 한 위치의 CLI 로그인 하나와 그 사용량. loom은 이 로그인을 여럿 두지 않고 바꾸지 않는다(M18 v2, Q666 A).
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaceUsage {
    /// WSL 배포판. 이 PC면 없다.
    pub distro: Option<String>,
    pub usage: Usage,
}

/// 로그인 창이 로그인할 곳.
#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
#[ts(export)]
pub enum LoginTarget {
    /// loom 계정을 새로 만든다. 로그인이 실패하거나 창을 닫으면 만든 폴더를 지운다.
    New,
    /// 그 loom 계정에 다시 로그인한다.
    Account(AccountId),
    /// 시스템 기본(그 CLI가 `HOME` 아래에서 고르는 로그인 폴더)에 로그인한다.
    System,
}
