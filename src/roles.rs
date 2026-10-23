//! o-mode 역할: 이름, 책임, 띄울 에이전트·모델·추론 강도. 설정(`SettingsView.roles`)에 두고 `loom spawn --role`이 읽는다.
//! 근거: docs/design.md 「설정」, 목업 docs/mockups/role-settings.html M12(Q510 A).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::profile::{AgentId, ModelChoice, accepts_choice};

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

/// 옛 설정에는 없다. 없거나 초기화하면 skills/o-mode에 있던 표다.
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
                "검토자",
                "병합 전 브랜치 diff에서 결함을 찾고 랩터-3이 잘 적용되었는지 점검한다. 결과·원문·관찰 증거(캡처, 평가 출력, 측정, 시험 데몬 결과)를 브리프의 완료 조건과 대조한다. 읽기 전용, 보고만 한다",
                Some(("codex", "gpt-6.1-sol", "high")),
            ),
            role(
                "CI 담당자",
                "병합 관문(워커 브랜치 검토자·반려), 검사, main 병합·push, 배포·릴리스 확인. 제품 코드는 고치지 않는다",
                Some(("claude", "claude-sonnet-5-5", "medium")),
            ),
        ])
    }
}

/// `loom spawn`이 띄울 에이전트와 지시문. 띄울 때 한 번 정한다. 뒤에 설정이 바뀌어도 뜬 노드는 그대로다.
#[derive(Debug, PartialEq)]
pub struct Assignment {
    pub agent: AgentId,
    pub choice: ModelChoice,
    pub brief: String,
}

#[derive(Debug, PartialEq)]
pub enum RoleProblem {
    Unknown { role: String, names: Vec<String> },
    NoAgent { role: String },
}

impl std::fmt::Display for RoleProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleProblem::Unknown { role, names } => write!(
                f,
                "역할 '{role}'이 설정에 없다. 있는 역할: {}",
                names.join(", ")
            ),
            RoleProblem::NoAgent { role } => write!(
                f,
                "역할 '{role}'에는 에이전트가 없다. --agent로 에이전트를 준다."
            ),
        }
    }
}

impl Roles {
    /// 띄울 때 `choice_args`가 거절할 역할의 문장. 사람이 저장할 때(`Settings::patch`)만 본다.
    /// 읽을 때 보지 않는 이유: 카탈로그가 바뀐 판에서 이미 저장한 설정 전체를 읽지 못하게 된다. 그때는 띄울 때 거절한다.
    pub fn launch_problem(&self) -> Option<String> {
        self.0.iter().find_map(|role| {
            let agent = role.agent.as_ref()?;
            let choice = ModelChoice {
                model: role.model.clone(),
                effort: role.effort.clone(),
            };
            (!accepts_choice(agent, &choice)).then(|| {
                format!(
                    "역할 '{}': 이 에이전트는 이 모델·추론 강도를 받지 않는다. Cursor는 추론 강도만 줄 수 없다. 모델을 준다.",
                    role.name
                )
            })
        })
    }

    /// 역할의 에이전트·모델·강도를 채운다. 명령에 준 값이 이긴다.
    /// 역할의 모델·강도는 역할의 에이전트로 띄울 때만 쓴다. 다른 에이전트는 그 모델을 받지 못한다.
    pub fn assign(
        &self,
        name: &str,
        agent: Option<AgentId>,
        choice: ModelChoice,
        brief: &str,
    ) -> Result<Assignment, RoleProblem> {
        let role = self
            .0
            .iter()
            .find(|r| r.name == name.trim())
            .ok_or_else(|| RoleProblem::Unknown {
                role: name.into(),
                names: self.0.iter().map(|r| r.name.clone()).collect(),
            })?;
        let agent = agent
            .or_else(|| role.agent.clone())
            .ok_or_else(|| RoleProblem::NoAgent {
                role: role.name.clone(),
            })?;
        let own = role.agent.as_ref() == Some(&agent);
        let from_role = |value: &Option<String>| value.clone().filter(|_| own);
        Ok(Assignment {
            agent,
            choice: ModelChoice {
                model: choice.model.or_else(|| from_role(&role.model)),
                effort: choice.effort.or_else(|| from_role(&role.effort)),
            },
            brief: format!("Role: {} - {}\n\n{brief}", role.name, role.duty),
        })
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
        let got = Roles::default()
            .assign("워커", None, ModelChoice::default(), "do it")
            .unwrap();
        assert_eq!(got.agent, AgentId("claude".into()));
        assert_eq!(
            got.choice,
            choice(Some("claude-sonnet-5-5"), Some("medium"))
        );
        assert_eq!(
            got.brief,
            "Role: 워커 - 정해진 편집 범위의 구현과 검증. 다시 위임하지 않는다\n\ndo it"
        );
    }

    #[test]
    fn flags_win_and_another_agent_drops_the_role_model() {
        let roles = Roles::default();
        let got = roles
            .assign("워커", None, choice(None, Some("high")), "")
            .unwrap();
        assert_eq!(got.choice, choice(Some("claude-sonnet-5-5"), Some("high")));
        let got = roles
            .assign(
                "워커",
                Some(AgentId("claude".into())),
                choice(Some("claude-opus-5-5"), None),
                "",
            )
            .unwrap();
        assert_eq!(got.choice, choice(Some("claude-opus-5-5"), Some("medium")));
        let got = roles
            .assign(
                "워커",
                Some(AgentId("codex".into())),
                ModelChoice::default(),
                "",
            )
            .unwrap();
        assert_eq!(got.agent, AgentId("codex".into()));
        assert_eq!(got.choice, ModelChoice::default());
    }

    #[test]
    fn unknown_role_and_role_without_agent_are_refused() {
        let roles = Roles::default();
        let err = roles
            .assign("없는역할", None, ModelChoice::default(), "")
            .unwrap_err();
        assert!(err.to_string().contains("총괄, 코디, 워커"), "{err}");
        let err = roles
            .assign("총괄", None, ModelChoice::default(), "")
            .unwrap_err();
        assert_eq!(
            err,
            RoleProblem::NoAgent {
                role: "총괄".into()
            }
        );
        assert!(err.to_string().contains("--agent"));
        let got = roles
            .assign(
                "총괄",
                Some(AgentId("claude".into())),
                ModelChoice::default(),
                "",
            )
            .unwrap();
        assert_eq!(got.choice, ModelChoice::default());
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
