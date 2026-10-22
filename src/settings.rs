//! 설정: 모든 권한 허용을 끈 에이전트, 공급자마다 쓰는 계정, 화면 테마, 터미널 테마·글꼴, 카드 보기, 그래프 범위, 간선 모양, 화면 언어,
//! 워크스페이스 폴더, 노드 나무 상한, 화면 색. SQLite에 한 줄로 둔다.
//! 근거: docs/design.md 「설정」.

use std::collections::BTreeMap;
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
    /// 옛 설정에는 없다. 없으면 `Line`(한 줄 알약)으로 읽는다.
    #[serde(default)]
    pub question_pill: QuestionPill,
    /// 옛 설정에는 없다. 없으면 `System`으로 읽는다.
    #[serde(default)]
    pub language: Language,
    /// 새 워크트리를 만드는 폴더. 데몬이 보는 절대 경로다. 없으면 기본값이고, 데몬이 화면에 보낼 때 채운다.
    pub workspace_dir: Option<String>,
    /// WSL 저장소의 새 워크트리를 만드는 폴더. 배포판 이름마다 UNC 경로다. 없는 배포판은 그 홈의 `loom/workspaces`다.
    /// WSL git은 Windows 폴더에 워크트리를 만들지 못해 위치마다 둔다. 옛 설정에는 없다. 근거: 목업 docs/mockups/wsl-place.html ⑤.
    pub wsl_workspace_dirs: BTreeMap<String, String>,
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
    /// 사람이 고른 화면 색. 옛 설정에는 없다. 없으면 모든 색이 기본값이다. 근거: 목업 docs/mockups/color-settings.html v3.
    #[serde(default)]
    pub colors: UiColors,
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

/// 그래프 위 질문 알약의 꼴. `Line`은 한 줄이고 목록을 펼쳐 답한다. `Answer`는 가장 오래된 질문 하나와 답 칸을 펼쳐 보인다(목업 M31 ②).
/// 근거: 2026-10-22 사람 지시 "질문 다시 알약으로 바꿔", "알약으로 할지 펼칠지는 설정에 옵션으로 제공".
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum QuestionPill {
    #[default]
    Line,
    Answer,
}

/// 뜻마다 사람이 고른 색. 라이트·다크를 따로 고른다(목업 docs/mockups/color-themes.html v3). 테마마다 쓸 색은 화면이 고른 색에서 만든다(`web/src/uiColors.ts`).
/// 칸 이름은 화면의 CSS 토큰(`web/src/styles.css`)과 짝이다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct UiColors {
    /// 질문·응답 대기(`--warn`).
    pub warn: ThemedColor,
    /// 실행 중(`--run`).
    pub run: ThemedColor,
    /// 완료·성공(`--ok`).
    pub ok: ThemedColor,
    /// 오류(`--err`).
    pub err: ThemedColor,
    /// 관계선(`--edge`).
    pub edge: ThemedColor,
    /// 저장소 상자(`--repo-line`).
    pub repo: ThemedColor,
    /// 워크트리 묶음(`--group-line`).
    pub worktree: ThemedColor,
    /// 강조(`--accent`).
    pub accent: ThemedColor,
}

/// 한 뜻의 테마별 색. `None`은 그 테마의 기본 색이다. 두 테마에 같은 색을 쓰면 두 칸이 같다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, TS)]
#[ts(export)]
pub struct ThemedColor {
    pub light: Option<HexColor>,
    pub dark: Option<HexColor>,
}

/// 옛 설정은 뜻마다 두 테마에 쓰는 색 하나(`"#rrggbb"`)를 저장했다. 그 값은 두 테마에 같은 색으로 읽는다.
impl<'de> Deserialize<'de> for ThemedColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Both(HexColor),
            Themes {
                #[serde(default)]
                light: Option<HexColor>,
                #[serde(default)]
                dark: Option<HexColor>,
            },
        }
        Ok(match Stored::deserialize(d)? {
            Stored::Both(hex) => Self {
                light: Some(hex.clone()),
                dark: Some(hex),
            },
            Stored::Themes { light, dark } => Self { light, dark },
        })
    }
}

/// `#rrggbb` 꼴의 색. 다른 꼴은 읽지 않는다. 그래서 틀린 색이 설정에 들어가지 않는다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, TS)]
#[ts(export, type = "string")]
pub struct HexColor(String);

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let hex = text.strip_prefix('#').unwrap_or("");
        if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(serde::de::Error::custom(format!(
                "색은 #rrggbb 꼴이다: {text}"
            )))
        }
    }
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
        Ok(read(&self.db.lock().unwrap())?.0)
    }

    /// 화면이 바꾼 칸만 받아 지금 설정에 합친다(JSON Merge Patch, RFC 7386: 객체는 칸마다 합치고 다른 값은 바꾼다).
    /// 읽기·합치기·쓰기를 한 잠금과 한 트랜잭션에서 한다. 그래서 두 화면이 같은 때 다른 칸을 바꿔도 둘 다 남는다.
    /// 합친 값이 `SettingsView`가 아니면 쓰지 않는다. 합친 설정을 돌려준다.
    pub fn patch(&self, patch: &serde_json::Value) -> Result<SettingsView, PatchProblem> {
        self.write(|_, value| {
            merge(value, patch);
            serde_json::from_value(value.clone()).map_err(|e| PatchProblem::Shape(e.to_string()))
        })
    }

    /// 데몬이 설정 하나를 바꾼다(예: 자동 전환한 계정). `patch`와 같은 잠금 안에서 읽고 쓴다.
    pub fn update(&self, edit: impl FnOnce(&mut SettingsView)) -> rusqlite::Result<()> {
        let result = self.write(|mut view, value| {
            edit(&mut view);
            *value = serde_json::to_value(&view).expect("설정은 늘 직렬화된다");
            Ok(view)
        });
        match result {
            Ok(_) => Ok(()),
            Err(PatchProblem::Db(e)) => Err(e),
            Err(PatchProblem::Shape(_)) => unreachable!("데몬이 만든 설정은 늘 맞는 모양이다"),
        }
    }

    fn write(
        &self,
        change: impl FnOnce(SettingsView, &mut serde_json::Value) -> Result<SettingsView, PatchProblem>,
    ) -> Result<SettingsView, PatchProblem> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let (view, mut value) = read(&tx)?;
        let next = change(view, &mut value)?;
        tx.execute(
            "INSERT OR REPLACE INTO settings (id, value) VALUES (1, ?1)",
            [serde_json::to_string(&next).expect("설정은 늘 직렬화된다")],
        )?;
        tx.commit()?;
        Ok(next)
    }
}

/// 설정 한 줄과 그 JSON. 형식이 옛것이면 기본값으로 시작한다. 다음 저장이 덮어쓴다.
fn read(db: &Connection) -> rusqlite::Result<(SettingsView, serde_json::Value)> {
    let text: Option<String> = db
        .query_row("SELECT value FROM settings WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()?;
    let view: SettingsView = text
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let value = serde_json::to_value(&view).expect("설정은 늘 직렬화된다");
    Ok((view, value))
}

/// RFC 7386. `null`은 칸을 지운다. 지운 칸은 읽을 때 기본값(없음·꺼짐)이 된다.
fn merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
    let serde_json::Value::Object(fields) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = serde_json::Value::Object(serde_json::Map::new());
    }
    let map = target.as_object_mut().expect("바로 위에서 객체로 만들었다");
    for (key, value) in fields {
        if value.is_null() {
            map.remove(key);
        } else {
            merge(
                map.entry(key.clone()).or_insert(serde_json::Value::Null),
                value,
            );
        }
    }
}

#[derive(Debug)]
pub enum PatchProblem {
    Db(rusqlite::Error),
    /// 합친 값이 설정 모양이 아니다. 화면이 보낸 칸이 틀렸다.
    Shape(String),
}

impl From<rusqlite::Error> for PatchProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
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
        settings.update(|v| *v = view.clone()).unwrap();
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
        assert_eq!(got.colors, UiColors::default());
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
        settings.update(|v| *v = view.clone()).unwrap();
        let got = settings.get().unwrap();
        assert_eq!(got.arrange_direction, ArrangeDirection::Left);
        assert!(got.minimap_folded);
        assert_eq!(got.repo, Some(RepoId(3)));
        assert_eq!(got.pane_widths, view.pane_widths);
        assert_eq!(got.closed_panes, view.closed_panes);
        assert_eq!(got.closed_switch_notice, Some(9));
    }

    /// 두 화면이 다른 칸을 따로 보낸다. 앞 쓰기가 뒤 쓰기에 지워지지 않는다. 안쪽 객체도 칸마다 합친다.
    #[test]
    fn partial_writes_of_different_fields_both_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::new(crate::db::open(&tmp.path().join("t.db")).unwrap());
        settings
            .patch(&serde_json::json!({"minimapFolded": true, "paneWidths": {"side": 300}}))
            .unwrap();
        settings
            .patch(&serde_json::json!({"arrangeDirection": "left", "paneWidths": {"dock": 400}}))
            .unwrap();
        let got = settings.get().unwrap();
        assert!(got.minimap_folded);
        assert_eq!(got.arrange_direction, ArrangeDirection::Left);
        assert_eq!(
            got.pane_widths,
            PaneWidths {
                side: Some(300),
                dock: Some(400)
            }
        );
        // `null`은 칸을 지워 기본값으로 돌린다.
        settings
            .patch(&serde_json::json!({"paneWidths": {"side": null}}))
            .unwrap();
        assert_eq!(settings.get().unwrap().pane_widths.side, None);
    }

    /// 모양이 틀린 칸은 쓰지 않는다.
    #[test]
    fn wrong_shape_is_refused_and_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::new(crate::db::open(&tmp.path().join("t.db")).unwrap());
        settings
            .patch(&serde_json::json!({"minimapFolded": true}))
            .unwrap();
        assert!(matches!(
            settings.patch(&serde_json::json!({"minimapFolded": "yes"})),
            Err(PatchProblem::Shape(_))
        ));
        assert!(settings.get().unwrap().minimap_folded);
    }

    /// 색은 `#rrggbb`만 받고, `null`은 그 칸을 기본값으로 돌린다. 테마마다 따로 바꾸고 따로 되돌린다.
    #[test]
    fn colors_take_only_hex_and_null_resets() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings::new(crate::db::open(&tmp.path().join("t.db")).unwrap());
        let hex = |h: &str| Some(HexColor(h.into()));
        let got = settings
            .patch(
                &serde_json::json!({"colors": {"warn": {"light": "#8B5CF6", "dark": "#8b5cf6"}}}),
            )
            .unwrap();
        assert_eq!(
            got.colors.warn,
            ThemedColor {
                light: hex("#8b5cf6"),
                dark: hex("#8b5cf6")
            }
        );
        let got = settings
            .patch(&serde_json::json!({"colors": {"warn": {"dark": "#eab308"}}}))
            .unwrap();
        assert_eq!(
            got.colors.warn,
            ThemedColor {
                light: hex("#8b5cf6"),
                dark: hex("#eab308")
            }
        );
        for bad in ["purple", "#8b5cf", "8b5cf6", "#8b5cfg"] {
            assert!(matches!(
                settings.patch(&serde_json::json!({"colors": {"edge": {"light": bad}}})),
                Err(PatchProblem::Shape(_))
            ));
        }
        let got = settings
            .patch(&serde_json::json!({"colors": {"warn": {"light": null}}}))
            .unwrap();
        assert_eq!(
            got.colors.warn,
            ThemedColor {
                light: None,
                dark: hex("#eab308")
            }
        );
        let got = settings
            .patch(&serde_json::json!({"colors": {"warn": null}}))
            .unwrap();
        assert_eq!(got.colors, UiColors::default());
    }

    /// 옛 설정의 한 색(`"#rrggbb"`)은 두 테마에 같은 색이다. 다음 패치가 한 테마만 바꿔도 다른 테마의 색이 남는다.
    #[test]
    fn old_single_color_reads_as_both_themes() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::db::open(&tmp.path().join("t.db")).unwrap();
        db.execute(
            "INSERT INTO settings (id, value) VALUES (1, ?1)",
            [r##"{"colors": {"warn": "#8b5cf6"}}"##],
        )
        .unwrap();
        let settings = Settings::new(db);
        let both = Some(HexColor("#8b5cf6".into()));
        assert_eq!(
            settings.get().unwrap().colors.warn,
            ThemedColor {
                light: both.clone(),
                dark: both.clone()
            }
        );
        let got = settings
            .patch(&serde_json::json!({"colors": {"warn": {"dark": "#eab308"}}}))
            .unwrap();
        assert_eq!(got.colors.warn.light, both);
    }
}
