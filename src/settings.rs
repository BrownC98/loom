use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::accounts::AccountId;
use crate::db::Database;
use crate::protocol::{AgentId, Provider, TreeLimits, WaitId};
use crate::roles::{ChoiceCheck, RoleProblem, Roles, SetRole};
use crate::skills::AlwaysOn;
use crate::worktree::RepoId;

#[derive(Clone, Default, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsView {
    /// 모든 권한 허용을 끈 에이전트. 여기 없는 에이전트는 승인 창 없이 모든 권한으로 뜬다.
    pub bypass_off: Vec<AgentId>,
    /// 이 PC 저장소의 새 노드가 공급자마다 쓰는 계정. 없거나 폴더가 사라졌으면 시스템 기본이다.
    pub accounts: InUse,
    /// WSL 배포판마다 그 배포판 저장소의 노드가 쓰는 계정. 키는 배포판 이름이다. 옛 설정에는 없다. 없는 배포판은 시스템 기본이다.
    /// 근거: 목업 docs/mockups/wsl-place.html ③ 「사용 중」은 위치마다 하나다.
    pub wsl_accounts: BTreeMap<String, InUse>,
    /// 옛 설정에는 없다. 없으면 `System`으로 읽고 다른 값은 그대로 둔다.
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub terminal_theme: TerminalTheme,
    #[serde(default)]
    pub terminal_font: TerminalFont,
    #[serde(default)]
    pub card_view: DefaultCardView,
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
    /// 근거: docs/design/에이전트-실행.md 「계정 전환」, 목업 docs/mockups/account-switch.html ②.
    #[serde(default)]
    pub auto_switch: bool,
    /// 화면이 가려져 있을 때 사람 응답 대기를 시스템 알림으로도 띄운다. 옛 설정에는 없다. 없으면 꺼짐이다.
    /// 받은편지함이 늘 보이므로 기본은 꺼짐이다. 근거: 목업 docs/mockups/human-question.html ⑥.
    #[serde(default)]
    pub os_notify: bool,
    /// 새 노드에 주는 내장 스킬. 옛 설정에는 없다. 없으면 모두 켜짐이다. 근거: docs/design/에이전트-실행.md 「내장 스킬」.
    #[serde(default)]
    pub always_on: AlwaysOn,
    /// 사람이 고른 화면 색. 옛 설정에는 없다. 없으면 모든 색이 기본값이다. 근거: 목업 docs/mockups/color-settings.html v3.
    #[serde(default)]
    pub colors: UiColors,
    /// o-mode 역할. 옛 설정에는 없다. 없거나 `null`이면 기본 표다. 근거: 목업 docs/mockups/role-settings.html M12.
    pub roles: Roles,
    /// 화면 상태(2026-10-21 사람 결정 「프론트는 뷰어」): 같은 데몬을 보는 모든 화면이 같은 값을 보인다. 아래 일곱 칸은 옛 설정에 없다.
    /// [정렬]과 나무 간선이 향하는 방향. 없으면 `Down`이다.
    pub arrange_direction: ArrangeDirection,
    /// 상시 정렬: 모든 저장소가 늘 정렬한 모양을 지킨다. 끌어 놓아도 형제 순서만 바뀐다. 없으면 꺼짐이다.
    /// 2026-10-22 사람 지시 "사이드바에 상시 정렬 기능 토글 버튼 추가", 목업 docs/mockups/always-arrange.html.
    pub always_arrange: bool,
    /// 그래프 미니맵을 접었는지. 없으면 펼침이다.
    pub minimap_folded: bool,
    /// 사이드바에서 고른 저장소. 없거나 목록에 없으면 화면이 첫 저장소를 쓴다.
    pub repo: Option<RepoId>,
    pub pane_widths: PaneWidths,
    /// 사람이 끌어 옮긴 질문 알약의 자리. 없으면 그래프 칸 위 가운데다.
    pub pill_place: Option<PillPlace>,
    pub closed_panes: ClosedPanes,
    /// 사용자가 닫은 계정 전환 알림의 이벤트 기록 번호.
    pub closed_switch_notice: Option<u32>,
    /// 사람이 알약의 [×]를 눌렀을 때 있던 가장 큰 대기 번호. 이보다 큰 대기가 오면 알약이 다시 뜬다.
    /// 근거: 목업 docs/mockups/pill-close.html M19 ②·③.
    pub closed_pill: Option<WaitId>,
}

/// 한 위치에서 공급자마다 쓰는 계정. 없으면 시스템 기본이다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, TS)]
#[ts(export)]
pub struct InUse(BTreeMap<Provider, AccountId>);

impl<'de> Deserialize<'de> for InUse {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let stored = BTreeMap::<String, serde_json::Value>::deserialize(d)?;
        Ok(Self(
            stored
                .into_iter()
                .filter_map(|(provider, id)| {
                    Some((
                        serde_json::from_value(serde_json::Value::String(provider)).ok()?,
                        serde_json::from_value(id).ok()?,
                    ))
                })
                .collect(),
        ))
    }
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

/// 질문 알약의 자리. 알약이 그래프 칸 안에서 움직일 수 있는 폭·높이 중 얼마만큼 왔는지(0~1)다.
/// 창이나 칸 크기가 바뀌어도 알약은 칸 안에 남는다.
#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct PillPlace {
    pub x: f32,
    pub y: f32,
}

impl PillPlace {
    fn problem(self) -> Option<String> {
        let ok = |v: f32| (0.0..=1.0).contains(&v);
        (!(ok(self.x) && ok(self.y))).then(|| "pillPlace must be numbers from 0 to 1".to_string())
    }
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

/// 노드에 따로 고른 카드 보기(`NodeView::card_view`)가 없을 때의 보기. `RootTerminal`은 부모 없는 노드만 표준 카드다
/// (목업 docs/mockups/node-card-view.html, 2026-10-23 사람 지시 "설정 옵션에 총괄만 터미널 옵션 제시").
/// 옛 이름(`terminal`, `header`)은 DB 12단계가 바꾼다.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DefaultCardView {
    #[default]
    Standard,
    Simple,
    RootTerminal,
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

/// 그래프 위 질문 알약의 꼴. `Line`은 한 줄이고 목록을 펼쳐 답한다. `Answer`는 가장 오래된 질문 하나와 답 칸을 펼쳐 보인다(목업 M31 ②). `Off`는 알약을 보이지 않는다(목업 M19 ④).
/// 근거: 2026-10-22 사람 지시 "질문 다시 알약으로 바꿔", "알약으로 할지 펼칠지는 설정에 옵션으로 제공", 2026-10-24 "알림 알약 끄는 기능 추가".
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum QuestionPill {
    #[default]
    Line,
    Answer,
    Off,
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
    /// 알림 수 배지(`--notice`). 질문과 완료를 함께 센다(2026-10-22 사람 지시 "알림탭 배지에 쓸 공통 알림 색이 필요함").
    pub notice: ThemedColor,
}

/// 한 뜻의 테마별 색. `None`은 그 테마의 기본 색이다. 두 테마에 같은 색을 쓰면 두 칸이 같다.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, TS)]
#[ts(export)]
pub struct ThemedColor {
    pub light: Option<HexColor>,
    pub dark: Option<HexColor>,
}

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
    pub fn account(&self, provider: Provider, distro: Option<&str>) -> Option<&AccountId> {
        let in_use = match distro {
            None => &self.accounts,
            Some(d) => self.wsl_accounts.get(d)?,
        };
        in_use.0.get(&provider)
    }

    pub fn set_account(&mut self, provider: Provider, distro: Option<&str>, id: Option<AccountId>) {
        let in_use = match distro {
            None => &mut self.accounts,
            Some(d) => self.wsl_accounts.entry(d.to_owned()).or_default(),
        };
        match id {
            Some(id) => in_use.0.insert(provider, id),
            None => in_use.0.remove(&provider),
        };
    }
}

pub struct Settings {
    db: Database,
    check: ChoiceCheck,
}

impl Settings {
    pub fn new(db: Database, check: ChoiceCheck) -> Self {
        Self { db, check }
    }

    pub fn get(&self) -> rusqlite::Result<SettingsView> {
        Ok(read(&self.db.lock())?.0)
    }

    pub fn patch(&self, patch: &serde_json::Value) -> Result<SettingsView, PatchProblem> {
        self.write(|_, value| {
            merge(value, patch);
            checked(value, self.check).map_err(PatchProblem::Shape)
        })
    }

    pub fn set_role(&self, set: &SetRole, known: ChoiceCheck) -> Result<Roles, SetRoleProblem> {
        self.write(|view, value| {
            let next = view.roles.with(set, known).map_err(SetRoleProblem::Role)?;
            merge(value, &serde_json::json!({ "roles": next }));
            checked(value, self.check).map_err(SetRoleProblem::Shape)
        })
        .map(|view| view.roles)
    }

    pub fn update(&self, edit: impl FnOnce(&mut SettingsView)) -> rusqlite::Result<()> {
        let result = self.write::<PatchProblem>(|mut view, value| {
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

    pub fn move_into_wsl(&self, distro: &str) -> rusqlite::Result<()> {
        self.update(|view| {
            if let Some(dir) = view.workspace_dir.take_if(|d| d.starts_with('/')) {
                view.wsl_workspace_dirs
                    .entry(distro.to_owned())
                    .or_insert(dir);
            }
            let pc = std::mem::take(&mut view.accounts);
            let in_use = view.wsl_accounts.entry(distro.to_owned()).or_default();
            for (provider, id) in pc.0 {
                in_use.0.entry(provider).or_insert(id);
            }
        })
    }

    fn write<E: From<rusqlite::Error>>(
        &self,
        change: impl FnOnce(SettingsView, &mut serde_json::Value) -> Result<SettingsView, E>,
    ) -> Result<SettingsView, E> {
        let mut db = self.db.lock();
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

fn checked(value: &serde_json::Value, check: ChoiceCheck) -> Result<SettingsView, String> {
    let view: SettingsView = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let problem = view
        .roles
        .launch_problem(check)
        .or_else(|| view.pill_place.and_then(|p| p.problem()));
    match problem {
        Some(problem) => Err(problem),
        None => Ok(view),
    }
}

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
    Shape(String),
}

pub enum SetRoleProblem {
    Db(rusqlite::Error),
    Shape(String),
    Role(RoleProblem),
}

impl From<rusqlite::Error> for SetRoleProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<rusqlite::Error> for PatchProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}
