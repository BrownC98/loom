use serde::{Deserialize, Serialize};

pub use api::settings::roles::{REVIEWER_ROLES, Role, Roles};

use crate::protocol::{AgentId, LaunchError, ModelChoice};

pub type ChoiceCheck = fn(&AgentId, &ModelChoice) -> Result<(), LaunchError>;

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

pub fn with(roles: &Roles, set: &SetRole, check: ChoiceCheck) -> Result<Roles, RoleProblem> {
    let choice = ModelChoice {
        model: set.model.clone().filter(|v| !v.trim().is_empty()),
        effort: set.effort.clone().filter(|v| !v.trim().is_empty()),
    };
    check(&set.agent, &choice).map_err(RoleProblem::Launch)?;
    let name = set.role.trim();
    let mut next = roles.clone();
    let role = next
        .0
        .iter_mut()
        .find(|r| r.name == name)
        .ok_or_else(|| RoleProblem::Unknown {
            role: set.role.clone(),
            names: roles.0.iter().map(|r| r.name.clone()).collect(),
        })?;
    role.agent = Some(set.agent.clone());
    role.model = set.model.clone();
    role.effort = set.effort.clone();
    Ok(next)
}

pub fn launch_problem(roles: &Roles, check: ChoiceCheck) -> Option<String> {
    roles.0.iter().find_map(|role| {
            let agent = role.agent.as_ref()?;
            let choice = ModelChoice {
                model: role.model.clone(),
                effort: role.effort.clone(),
            };
            check(agent, &choice).is_err().then(|| {
                format!(
                    "역할 '{}': 이 에이전트는 이 모델·추론 강도를 받지 않는다. Cursor는 추론 강도만 줄 수 없다. 모델을 준다.",
                    role.name
                )
            })
        })
}

pub fn assign(roles: &Roles, name: &str, brief: &str) -> Result<Assignment, RoleProblem> {
    let role = roles
        .0
        .iter()
        .find(|r| r.name == name.trim())
        .ok_or_else(|| RoleProblem::Unknown {
            role: name.into(),
            names: roles.0.iter().map(|r| r.name.clone()).collect(),
        })?;
    let agent = role.agent.clone().ok_or_else(|| RoleProblem::NoAgent {
        role: role.name.clone(),
    })?;
    Ok(Assignment {
        choice: fill(role, &agent, ModelChoice::default()),
        agent,
        brief: format!("Role: {} - {}\n\n{brief}", role.name, role.duty),
    })
}

pub fn choice_for(roles: &Roles, name: &str, agent: &AgentId, choice: ModelChoice) -> ModelChoice {
    match roles.0.iter().find(|r| r.name == name.trim()) {
        Some(role) => fill(role, agent, choice),
        None => choice,
    }
}

fn fill(role: &Role, agent: &AgentId, choice: ModelChoice) -> ModelChoice {
    let own = role.agent.as_ref() == Some(agent);
    let from_role = |value: &Option<String>| value.clone().filter(|_| own);
    ModelChoice {
        model: choice.model.or_else(|| from_role(&role.model)),
        effort: choice.effort.or_else(|| from_role(&role.effort)),
    }
}
