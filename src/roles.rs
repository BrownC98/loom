use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::profile::{check_choice, check_known_choice};
use crate::protocol::{AgentId, LaunchError, ModelChoice};

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
                "검토자",
                "병합 전 브랜치 diff에서 결함을 찾고 랩터-3이 잘 적용되었는지 점검한다. 결과·원문·관찰 증거(캡처, 평가 출력, 측정, 시험 데몬 결과)를 브리프의 완료 조건과 대조한다. 읽기 전용, 보고만 한다",
                Some(("codex", "gpt-6.1-sol", "high")),
            ),
            role(
                "보조 검토자",
                "main 대상 브랜치를 검토자와 따로 검토한다. 병합 전 브랜치 diff에서 결함을 찾고 결과·원문·관찰 증거를 브리프의 완료 조건과 대조한다. 읽기 전용, 보고만 한다",
                Some(("claude", "claude-opus-5-5", "medium")),
            ),
        ])
    }
}

pub const HIGH_RISK_WORKER: &str = "고위험 워커";
pub const HIGH_RISK_DUTY: &str = "저장 형식, 인증, 동시성, 데이터 손실 경로, 여러 모듈의 계약을 바꾸는 편집 범위의 구현과 검증. 다시 위임하지 않는다";

#[derive(Debug, PartialEq)]
pub struct Assignment {
    pub agent: AgentId,
    pub choice: ModelChoice,
    pub brief: String,
}

#[derive(Debug)]
pub enum RoleProblem {
    Unknown { role: String, names: Vec<String> },
    NoAgent { role: String },
    Launch(LaunchError),
}

#[derive(Serialize, Deserialize)]
pub struct SetRole {
    pub role: String,
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl std::fmt::Display for RoleProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleProblem::Unknown { role, names } => write!(
                f,
                "역할 '{role}'이 설정에 없다. 있는 역할: {}",
                names.join(", ")
            ),
            RoleProblem::NoAgent { role } => {
                write!(
                    f,
                    "역할 '{role}'에는 에이전트가 없다. 이 역할은 사람이 띄운다."
                )
            }
            RoleProblem::Launch(e) => write!(f, "{e}"),
        }
    }
}

impl Roles {
    pub fn with(&self, set: &SetRole) -> Result<Roles, RoleProblem> {
        let choice = ModelChoice {
            model: set.model.clone().filter(|v| !v.trim().is_empty()),
            effort: set.effort.clone().filter(|v| !v.trim().is_empty()),
        };
        check_known_choice(&set.agent, &choice).map_err(RoleProblem::Launch)?;
        let name = set.role.trim();
        let mut next = self.clone();
        let role =
            next.0
                .iter_mut()
                .find(|r| r.name == name)
                .ok_or_else(|| RoleProblem::Unknown {
                    role: set.role.clone(),
                    names: self.0.iter().map(|r| r.name.clone()).collect(),
                })?;
        role.agent = Some(set.agent.clone());
        role.model = set.model.clone();
        role.effort = set.effort.clone();
        Ok(next)
    }

    pub fn launch_problem(&self) -> Option<String> {
        self.0.iter().find_map(|role| {
            let agent = role.agent.as_ref()?;
            let choice = ModelChoice {
                model: role.model.clone(),
                effort: role.effort.clone(),
            };
            check_choice(agent, &choice).is_err().then(|| {
                format!(
                    "역할 '{}': 이 에이전트는 이 모델·추론 강도를 받지 않는다. Cursor는 추론 강도만 줄 수 없다. 모델을 준다.",
                    role.name
                )
            })
        })
    }

    pub fn assign(&self, name: &str, brief: &str) -> Result<Assignment, RoleProblem> {
        let role = self
            .0
            .iter()
            .find(|r| r.name == name.trim())
            .ok_or_else(|| RoleProblem::Unknown {
                role: name.into(),
                names: self.0.iter().map(|r| r.name.clone()).collect(),
            })?;
        let agent = role.agent.clone().ok_or_else(|| RoleProblem::NoAgent {
            role: role.name.clone(),
        })?;
        Ok(Assignment {
            choice: role.fill(&agent, ModelChoice::default()),
            agent,
            brief: format!("Role: {} - {}\n\n{brief}", role.name, role.duty),
        })
    }

    pub fn choice_for(&self, name: &str, agent: &AgentId, choice: ModelChoice) -> ModelChoice {
        match self.0.iter().find(|r| r.name == name.trim()) {
            Some(role) => role.fill(agent, choice),
            None => choice,
        }
    }
}

impl Role {
    fn fill(&self, agent: &AgentId, choice: ModelChoice) -> ModelChoice {
        let own = self.agent.as_ref() == Some(agent);
        let from_role = |value: &Option<String>| value.clone().filter(|_| own);
        ModelChoice {
            model: choice.model.or_else(|| from_role(&self.model)),
            effort: choice.effort.or_else(|| from_role(&self.effort)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(model: Option<&str>, effort: Option<&str>) -> ModelChoice {
        ModelChoice {
            model: model.map(Into::into),
            effort: effort.map(Into::into),
        }
    }

    #[test]
    fn worker_role_takes_its_agent_model_effort_and_prefixes_the_brief() {
        let got = Roles::default().assign("워커", "do it").unwrap();
        assert_eq!(got.agent, AgentId("claude".into()));
        assert_eq!(
            got.choice,
            choice(Some("claude-sonnet-5-5"), Some("medium"))
        );
        assert_eq!(
            got.brief,
            "Role: 워커 - 정해진 편집 범위의 구현과 검증. 다시 위임하지 않는다\n\ndo it"
        );
        let got = Roles::default().assign(HIGH_RISK_WORKER, "").unwrap();
        assert_eq!(got.choice, choice(Some("claude-opus-5-5"), Some("high")));
    }

    #[test]
    fn relaunch_choice_follows_the_spawn_rule() {
        let roles = Roles::default();
        let claude = AgentId("claude".into());
        let cursor = AgentId("cursor".into());
        assert_eq!(
            roles.choice_for("코디", &claude, ModelChoice::default()),
            choice(Some("claude-opus-5-5"), Some("medium"))
        );
        assert_eq!(
            roles.choice_for("코디", &cursor, ModelChoice::default()),
            ModelChoice::default()
        );
        assert_eq!(
            roles.choice_for("없는역할", &claude, choice(None, Some("high"))),
            choice(None, Some("high"))
        );
    }

    #[test]
    fn unknown_role_and_role_without_agent_are_refused() {
        let roles = Roles::default();
        let err = roles.assign("없는역할", "").unwrap_err();
        assert!(err.to_string().contains("총괄, 코디, 워커"), "{err}");
        let err = roles.assign("총괄", "").unwrap_err();
        assert!(matches!(err, RoleProblem::NoAgent { role } if role == "총괄"));
    }

    #[test]
    fn empty_or_duplicate_names_are_refused_and_blank_model_is_none() {
        let parse = |v: serde_json::Value| serde_json::from_value::<Roles>(v);
        assert!(parse(serde_json::json!([{"name": " ", "duty": "", "agent": null, "model": null, "effort": null}])).is_err());
        assert!(
            parse(serde_json::json!([
                {"name": "a", "duty": "", "agent": null, "model": null, "effort": null},
                {"name": "a ", "duty": "", "agent": null, "model": null, "effort": null}
            ]))
            .is_err()
        );
        let got = parse(serde_json::json!([{"name": " b ", "duty": "d", "agent": "claude", "model": "", "effort": " "}])).unwrap();
        assert_eq!(got.0[0].name, "b");
        assert_eq!(got.0[0].model, None);
        assert_eq!(got.0[0].effort, None);
    }

    #[test]
    fn cursor_effort_without_model_is_refused_and_defaults_pass() {
        let parse = |model: Option<&str>, effort: Option<&str>| {
            serde_json::from_value::<Roles>(serde_json::json!([
                {"name": "a", "duty": "", "agent": "cursor", "model": model, "effort": effort}
            ]))
            .unwrap()
            .launch_problem()
        };
        assert_eq!(parse(None, None), None);
        assert_eq!(parse(Some("m"), None), None);
        assert_eq!(parse(Some("m"), Some("high")), None);
        let err = parse(None, Some("high")).unwrap();
        assert!(err.contains("Cursor"), "{err}");
        assert_eq!(Roles::default().launch_problem(), None);
        let defaults = serde_json::to_value(Roles::default()).unwrap();
        assert_eq!(
            serde_json::from_value::<Roles>(defaults).unwrap(),
            Roles::default()
        );
    }
}
