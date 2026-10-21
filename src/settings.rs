//! 설정: 모든 권한 허용을 끈 에이전트, 공급자마다 쓰는 계정, 화면 테마, 터미널 테마·글꼴, 카드 보기, 그래프 범위, 간선 모양, 화면 언어,
//! 워크스페이스 폴더, 노드 나무 상한. SQLite에 한 줄로 둔다.
//! 근거: docs/design.md 「설정」.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::accounts::AccountId;
use crate::graph::TreeLimits;
use crate::profile::{AgentId, Provider};
use crate::skills::AlwaysOn;
use crate::worktree::RepoId;

#[derive(Clone, Default, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsView {
    /// 모든 권한 허용을 끈 에이전트. 여기 없는 에이전트는 승인 창 없이 모든 권한으로 뜬다.
    pub bypass_off: Vec<AgentId>,
    /// 새 Claude 노드가 쓰는 계정. 없거나 폴더가 사라졌으면 시스템 기본이다.
    pub claude_account: Option<AccountId>,
    pub codex_account: Option<AccountId>,
    /// 옛 설정에는 없다. 없으면 `System`으로 읽고 다른 값은 그대로 둔다.
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub terminal_theme: TerminalTheme,
    #[serde(default)]
    pub terminal_font: TerminalFont,
    #[serde(default)]
    pub card_view: CardView,
    /// 옛 설정에는 없다. 없으면 `Repo`(저장소 하나)로 읽는다.
    #[serde(default)]
    pub graph_scope: GraphScope,
    /// 옛 설정에는 없다. 없으면 `Step`(계단)으로 읽는다.
    #[serde(default)]
    pub edge_style: EdgeStyle,
    /// 옛 설정에는 없다. 없으면 `System`으로 읽는다.
    #[serde(default)]
    pub language: Language,
    /// 새 워크트리를 만드는 폴더. 데몬이 보는 절대 경로다. 없으면 기본값이고, 데몬이 화면에 보낼 때 채운다.
    pub workspace_dir: Option<String>,
    /// 옛 설정에는 없다. 없으면 깊이 2, 자식 수 상한 없음으로 읽는다.
    #[serde(default)]
    pub tree_limits: TreeLimits,
    /// 쓰는 계정이 한도에 닿으면 같은 공급자의 남은 계정으로 바꾸고 실행 중인 노드도 옮긴다. 옛 설정에는 없다. 없으면 꺼짐이다.
    /// 근거: docs/design.md 「계정 전환」, 목업 docs/mockups/account-switch.html ②.
    #[serde(default)]
    pub auto_switch: bool,
    /// 화면이 가려져 있을 때 사람 응답 대기를 시스템 알림으로도 띄운다. 옛 설정에는 없다. 없으면 꺼짐이다.
    /// 받은편지함이 늘 보이므로 기본은 꺼짐이다. 근거: 목업 docs/mockups/human-question.html ⑥.
    #[serde(default)]
    pub os_notify: bool,
    /// 새 노드에 주는 내장 스킬. 옛 설정에는 없다. 없으면 모두 켜짐이다. 근거: docs/design.md 「내장 스킬」.
    #[serde(default)]
    pub always_on: AlwaysOn,
    /// 화면 상태(2026-10-21 사람 결정 「프론트는 뷰어」): 같은 데몬을 보는 모든 화면이 같은 값을 보인다. 아래 여섯 칸은 옛 설정에 없다.
    /// [정렬]과 나무 간선이 향하는 방향. 없으면 `Down`이다.
    pub arrange_direction: ArrangeDirection,
    /// 그래프 미니맵을 접었는지. 없으면 펼침이다.
    pub minimap_folded: bool,
    /// 사이드바에서 고른 저장소. 없거나 목록에 없으면 화면이 첫 저장소를 쓴다.
    pub repo: Option<RepoId>,
    pub pane_widths: PaneWidths,
    pub closed_panes: ClosedPanes,
    /// 사용자가 닫은 계정 전환 알림의 이벤트 기록 번호.
    pub closed_switch_notice: Option<u32>,
}

/// [정렬] 방향. 나무가 뻗는 쪽이다.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ArrangeDirection {
    #[default]
    Down,
    Right,
    Up,
    Left,
}

/// 사람이 끌어 바꾼 사이드바 너비(px). 없으면 화면이 칸의 기본 너비를 쓴다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct PaneWidths {
    pub side: Option<u16>,
    pub dock: Option<u16>,
}

/// 접은 칸(사이드바, 오른쪽 칸)과 절(북마크). 처음에는 모두 펼침이다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct ClosedPanes {
    pub side: bool,
    pub dock: bool,
    pub bookmarks: bool,
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

/// 모든 터미널이 쓰는 글꼴. 기본값은 화면(`web/src/Terminal.tsx`)에만 있다. 없으면 화면이 기본값을 쓴다.
/// Orca의 `terminalFontFamily`·`terminalFontSize`와 같은 자리다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalFont {
    /// CSS `font-family` 문자열. 글자마다 목록에서 그 글자가 있는 첫 글꼴을 쓴다(VS Code `terminal.integrated.fontFamily`).
    pub family: Option<String>,
    pub size: Option<u8>,
}

/// 그래프 전체의 카드 보기. `Standard`는 카드에 터미널이 있다. `Simple`은 카드에 머리 정보만 있고, 고른 노드의 터미널은 오른쪽 칸에만 열린다.
/// 옛 이름(`terminal`, `header`)은 DB 12단계가 바꾼다.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CardView {
    #[default]
    Standard,
    Simple,
}

/// 그래프 하나가 보이는 저장소. `Repo`는 사이드바에서 고른 저장소 하나, `All`은 더한 저장소 모두다.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GraphScope {
    #[default]
    Repo,
    All,
}

/// 그래프 나무 간선(자식이 나무가 뻗는 쪽에 있는 부모-자식 간선)의 모양. 다른 간선은 늘 곡선이다.
/// `Step`은 부모 카드 아래에서 꺾어 카드를 지나지 않는다(목업 docs/mockups/arrange-mixed.html v5 규칙 6). `Curve`는 곡선이다(2026-10-21 사람 지시 "꺽은선, 곡선은 옵션에서 선택할 수 있게 해줘").
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EdgeStyle {
    #[default]
    Step,
    Curve,
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

    pub fn set_account(&mut self, provider: Provider, id: Option<AccountId>) {
        match provider {
            Provider::Claude => self.claude_account = id,
            Provider::Codex => self.codex_account = id,
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
        assert!(settings.get().unwrap().bypass_off.is_empty());
        let view = SettingsView {
            bypass_off: vec![AgentId("codex".into())],
            ..Default::default()
        };
        settings.put(&view).unwrap();
        assert_eq!(settings.get().unwrap().bypass_off, view.bypass_off);
    }

    #[test]
    fn old_row_without_theme_keeps_other_values() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&tmp.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO settings (id, value) VALUES (1, ?1)",
            [r#"{"bypassOff":["codex"],"claudeAccount":null,"codexAccount":null}"#],
        )
        .unwrap();
        let got = Settings::new(conn).get().unwrap();
        assert_eq!(got.bypass_off, [AgentId("codex".into())]);
        assert_eq!(got.theme, Theme::System);
        assert_eq!(got.terminal_theme, TerminalTheme::default());
        assert_eq!(got.terminal_font, TerminalFont::default());
        assert_eq!(got.card_view, CardView::Standard);
        assert_eq!(got.graph_scope, GraphScope::Repo);
        assert_eq!(got.language, Language::System);
        assert_eq!(got.always_on, AlwaysOn::default());
        assert_eq!(got.arrange_direction, ArrangeDirection::Down);
        assert!(!got.minimap_folded);
        assert_eq!(got.repo, None);
        assert_eq!(got.pane_widths, PaneWidths::default());
        assert_eq!(got.closed_panes, ClosedPanes::default());
        assert_eq!(got.closed_switch_notice, None);
    }

    #[test]
    fn view_state_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::new(crate::db::open(&tmp.path().join("t.db")).unwrap());
        let view = SettingsView {
            arrange_direction: ArrangeDirection::Left,
            minimap_folded: true,
            repo: Some(RepoId(3)),
            pane_widths: PaneWidths {
                side: Some(300),
                dock: None,
            },
            closed_panes: ClosedPanes {
                bookmarks: true,
                ..Default::default()
            },
            closed_switch_notice: Some(9),
            ..Default::default()
        };
        settings.put(&view).unwrap();
        let got = settings.get().unwrap();
        assert_eq!(got.arrange_direction, ArrangeDirection::Left);
        assert!(got.minimap_folded);
        assert_eq!(got.repo, Some(RepoId(3)));
        assert_eq!(got.pane_widths, view.pane_widths);
        assert_eq!(got.closed_panes, view.closed_panes);
        assert_eq!(got.closed_switch_notice, Some(9));
    }
}
