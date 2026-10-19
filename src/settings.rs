//! 설정: 켠 에이전트, 기본 에이전트, 공급자마다 쓰는 계정. SQLite에 한 줄로 둔다.
//! 근거: docs/design.md 「설정」.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::accounts::AccountId;
use crate::profile::{AgentId, Profile};

#[derive(Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsView {
    /// 없으면 켠 에이전트 가운데 첫째를 쓴다.
    pub default_agent: Option<AgentId>,
    /// 끈 에이전트. 설치했어도 새 노드 창에 보이지 않는다.
    pub disabled_agents: Vec<AgentId>,
    /// 새 Claude 노드가 쓰는 계정. 없거나 폴더가 사라졌으면 시스템 기본이다.
    pub claude_account: Option<AccountId>,
    pub codex_account: Option<AccountId>,
}

impl SettingsView {
    pub fn account(&self, provider: Profile) -> Option<&AccountId> {
        match provider {
            Profile::Claude => self.claude_account.as_ref(),
            Profile::Codex => self.codex_account.as_ref(),
        }
    }
}

pub struct Settings {
    db: Mutex<Connection>,
}

impl Settings {
    pub fn new(conn: Connection) -> Self {
        Self {
            db: Mutex::new(conn),
        }
    }

    pub fn get(&self) -> rusqlite::Result<SettingsView> {
        let text: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT value FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        // 형식이 옛것이면 기본값으로 시작한다. 다음 저장이 덮어쓴다.
        Ok(text
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default())
    }

    pub fn put(&self, view: &SettingsView) -> rusqlite::Result<()> {
        let text = serde_json::to_string(view).expect("설정은 늘 직렬화된다");
        self.db.lock().unwrap().execute(
            "INSERT OR REPLACE INTO settings (id, value) VALUES (1, ?1)",
            [text],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::new(crate::db::open(&tmp.path().join("t.db")).unwrap());
        assert!(settings.get().unwrap().default_agent.is_none());
        let view = SettingsView {
            default_agent: Some(AgentId("codex".into())),
            disabled_agents: vec![AgentId("aider".into())],
            ..Default::default()
        };
        settings.put(&view).unwrap();
        let got = settings.get().unwrap();
        assert_eq!(got.default_agent, view.default_agent);
        assert_eq!(got.disabled_agents, view.disabled_agents);
    }
}
