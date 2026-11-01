use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::AgentId;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Role {
    pub name: String,
    /// 역할의 책임. 띄운 노드의 지시문 첫 줄이 된다.
    pub duty: String,
    /// 없으면 `loom spawn`이 띄우지 않는다(총괄은 사람이 띄운다).
    pub agent: Option<AgentId>,
    /// 없으면 CLI 설정의 모델·강도다. 빈 글은 `Roles`가 읽을 때 없음으로 바꾼다.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// 역할 목록. 이름은 비지 않고 서로 다르다. 읽을 때 막으므로 설정에 틀린 목록이 들어가지 않는다.
/// 하나짜리 튜플 구조체라 JSON은 배열 그대로다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, TS)]
#[ts(export)]
pub struct Roles(pub Vec<Role>);

impl<'de> Deserialize<'de> for Roles {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut roles = Vec::<Role>::deserialize(d)?;
        let mut seen = std::collections::HashSet::new();
        for role in &mut roles {
            role.name = role.name.trim().to_owned();
            for value in [&mut role.model, &mut role.effort] {
                *value = value.take().filter(|v| !v.trim().is_empty());
            }
            if role.name.is_empty() {
                return Err(serde::de::Error::custom("역할 이름이 비었다"));
            }
            if !seen.insert(role.name.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "역할 이름이 겹친다: {}",
                    role.name
                )));
            }
        }
        Ok(Self(roles))
    }
}

impl Default for Roles {
    fn default() -> Self {
        let role = |name: &str, duty: &str, run: Option<(&str, &str, &str)>| Role {
            name: name.into(),
            duty: duty.into(),
            agent: run.map(|(agent, _, _)| AgentId(agent.into())),
            model: run.map(|(_, model, _)| model.into()),
            effort: run.map(|(_, _, effort)| effort.into()),
        };
        Self(vec![
            role("총괄", "범위, 배정, 사용자 승인, 노드 회수", None),
            role(
                "코디",
                "작업 하나의 조사, 설계, 목업, 워커 명세와 결과 검수",
                Some(("claude", "claude-opus-5-5", "medium")),
            ),
            role(
                "워커",
                "정해진 편집 범위의 구현과 검증. 다시 위임하지 않는다",
                Some(("claude", "claude-sonnet-5-5", "medium")),
            ),
            role(
                HIGH_RISK_WORKER,
                HIGH_RISK_DUTY,
                Some(("claude", "claude-opus-5-5", "high")),
            ),
            role(
                REVIEWER,
                "병합 전 브랜치 diff에서 결함을 찾고 랩터-3이 잘 적용되었는지 점검한다. 결과·원문·관찰 증거(캡처, 평가 출력, 측정, 시험 데몬 결과)를 브리프의 완료 조건과 대조한다. 지시문의 결함 목록을 차례로 본다. 전체 검사는 돌리지 않고 담당의 검사 기록을 읽는다. 읽기 전용, 보고만 한다",
                Some(("codex", "gpt-6.1-sol", "high")),
            ),
            role(
                SECOND_REVIEWER,
                "main 대상 브랜치를 검토자와 따로 검토한다. 병합 전 브랜치 diff에서 결함을 찾고 결과·원문·관찰 증거를 브리프의 완료 조건과 대조한다. 지시문의 결함 목록을 차례로 본다. 전체 검사는 돌리지 않고 담당의 검사 기록을 읽는다. 읽기 전용, 보고만 한다",
                Some(("claude", "claude-opus-5-5", "medium")),
            ),
        ])
    }
}

const HIGH_RISK_WORKER: &str = "고위험 워커";
const HIGH_RISK_DUTY: &str = "저장 형식, 인증, 동시성, 데이터 손실 경로, 여러 모듈의 계약을 바꾸는 편집 범위의 구현과 검증. 다시 위임하지 않는다";
const REVIEWER: &str = "검토자";
const SECOND_REVIEWER: &str = "보조 검토자";
pub const REVIEWER_ROLES: [&str; 2] = [REVIEWER, SECOND_REVIEWER];
