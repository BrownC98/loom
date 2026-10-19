//! 설정: 켠 에이전트, 기본 에이전트, 공급자마다 쓰는 계정, 화면 테마, 터미널 테마, 화면 언어. SQLite에 한 줄로 둔다.
//! 근거: docs/design.md 「설정」.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::accounts::AccountId;
use crate::profile::{AgentId, Provider};

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
    /// 옛 설정에는 없다. 없으면 `System`으로 읽고 다른 값은 그대로 둔다.
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub terminal_theme: TerminalTheme,
    /// 옛 설정에는 없다. 없으면 `System`으로 읽는다.
    #[serde(default)]
    pub language: Language,
}

/// 화면 언어. `System`은 브라우저 언어(`navigator.language`)를 따른다. Orca의 `uiLanguage`와 같은 자리다.
/// 에이전트가 읽는 글(CLI 출력, 노드 메시지)은 이 값과 관계없이 한국어다.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Language {
    #[default]
    System,
    En,
    Ko,
}

/// 모드마다 쓰는 터미널 테마 이름. 테마 목록과 기본값은 화면(`web/src/terminalThemes.ts`)에만 있다.
/// 이름이 없거나 목록에 없으면 화면이 기본값을 쓴다. 초기화는 이 값을 기본값(빈 값)으로 바꾼다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalTheme {
    pub dark: Option<String>,
    pub light: Option<String>,
    /// 켜면 라이트 모드에서도 다크 모드 테마를 쓴다.
    pub match_dark: bool,
}

/// 화면 테마. `System`은 OS 설정(`prefers-color-scheme`)을 따른다.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl SettingsView {
    pub fn account(&self, provider: Provider) -> Option<&AccountId> {
        match provider {
            Provider::Claude => self.claude_account.as_ref(),
            Provider::Codex => self.codex_account.as_ref(),
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

    #[test]
    fn old_row_without_theme_keeps_other_values() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&tmp.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO settings (id, value) VALUES (1, ?1)",
            [r#"{"defaultAgent":"codex","disabledAgents":[],"claudeAccount":null,"codexAccount":null}"#],
        )
        .unwrap();
        let got = Settings::new(conn).get().unwrap();
        assert_eq!(got.default_agent, Some(AgentId("codex".into())));
        assert_eq!(got.theme, Theme::System);
        assert_eq!(got.terminal_theme, TerminalTheme::default());
        assert_eq!(got.language, Language::System);
    }
}
