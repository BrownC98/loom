use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{ClosedBy, Who};
use super::text::TextPart;
use crate::ids::WaitId;

/// 사람이 할 일의 종류. 화면이 이름과 아이콘을 고른다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WaitKind {
    /// 에이전트가 고르기·답을 묻는다.
    Question,
    /// 명령·도구·계획을 승인해야 한다.
    Approval,
    /// 폴더 신뢰를 확인해야 한다.
    Trust,
    /// 로그인이나 계정 한도 선택이 필요하다.
    Auth,
    /// 터미널 프로그램이 입력(비밀번호, y/n)을 기다린다.
    Input,
    /// 사람이 띄운 노드가 맡은 일을 끝냈다(`loom done --asked`). 사람은 [확인]하거나 이어서 맡긴다.
    Done,
}

/// 질문·완료에 붙은 문서(user-doc 목업·보고서 HTML). 화면은 ID와 제목만 받고 문서 페이지 `/docs/{id}`로 연다. 파일 경로는 데몬의 문서 ID 표(`user_doc::doc_file`)만 안다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitDoc {
    /// `M25`, `R3` 같은 문서 ID.
    pub id: String,
    pub title: String,
}

/// 터미널 창(승인, 질문 상자, 폴더 신뢰, 로그인, 셸 입력)의 본문과 답하는 수단(`profile::prompt`). 화면은 선택지마다 버튼을, `field`가 있으면 입력칸을 그리고, 누르면 이 값을 그대로 돌려보낸다(`Graph::dialog`). 2026-10-24 목업 M20 v2 승인(Q889).
/// 데몬은 보내기 직전 화면을 다시 읽어 같은 값일 때만 키를 친다. 그래서 다른 창에 키가 들어가지 않는다(2026-10-23 목업 M11 승인).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Prompt {
    /// 창 위부터 마지막 선택지까지 화면 줄 그대로. 같은 창인지는 이 줄들이 정한다. `body`는 보기 좋게 다듬은 글이라 다른 명령이 같은 글이 될 수 있다(2026-10-23 리뷰: 명령 줄 「Tip:; touch one」).
    pub rows: Vec<String>,
    /// 창 본문(도구, 명령, 이유) 줄들. 명령 줄은 앞 공백까지 터미널 글 그대로다.
    pub body: Vec<String>,
    /// 무엇을 승인하는지 한 줄(명령). 입력 창 대상 목록 줄과 시스템 알림이 쓴다.
    pub summary: String,
    pub options: Vec<PromptOption>,
    /// 셸 입력 물음(비밀번호, 이름)만: 글을 쳐서 답하는 칸. 데몬은 보낸 글과 Enter를 치고 그 글을 어디에도 두지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub field: Option<InputField>,
}

/// 셸 입력칸의 종류. `Secret`은 화면이 글자를 가린다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InputField {
    Text,
    Secret,
}

/// 승인 창의 선택지 하나. `label`은 터미널 글(영어) 그대로다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptOption {
    pub label: String,
    pub key: PromptKey,
    /// 글로 답하는 선택지(예: 「Other」). 화면은 입력칸을 붙이고, 데몬은 이 선택지로 옮긴 뒤 글과 Enter를 친다(2026-10-27 목업 M29 v1 승인).
    #[serde(default)]
    pub writes: bool,
}

/// 선택지를 고르는 키. `Char`·`Tab`·`BackTab`은 누르면 바로 정한다(실측: 숫자 키 하나, Enter 없음).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum PromptKey {
    Char(char),
    Tab,
    BackTab,
    /// 터미널 키가 아니다. 에이전트 어댑터가 읽는 답이다.
    #[serde(rename = "permission")]
    Reply(String),
    /// 번호 없는 목록: 고른 줄에서 이만큼 화살표로 옮긴 뒤 Enter. 아래가 양수다(숫자 키를 받지 않는 창이 있다).
    Move(i8),
    /// 체크 목록: 고른 줄에서 이만큼 화살표로 옮긴 뒤 Space로 고르고 Enter(2026.10.22 질문 상자 실측: Enter만으로는 보내지 않는다).
    Tick(i8),
    /// 셸 물음: 이 글과 Enter.
    Line(String),
    /// 선택지 없이 아무 키나 기다리는 창(「Press any key to log in」): Enter.
    Enter,
}

pub type Strokes = Vec<Vec<u8>>;

const ENTER: &[u8] = b"\r";

impl PromptKey {
    pub fn picked(&self) -> Option<Strokes> {
        let tail: &[&[u8]] = match self {
            PromptKey::Move(_) | PromptKey::Line(_) => &[ENTER],
            PromptKey::Tick(_) => &[b" ", ENTER],
            _ => &[],
        };
        Some([self.reached()?, tail.iter().map(|k| k.to_vec()).collect()].concat())
    }

    pub fn written(&self, text: &str) -> Option<Strokes> {
        match self {
            PromptKey::Line(_) | PromptKey::Enter => None,
            _ => Some([self.reached()?, entered(text)].concat()),
        }
    }

    fn reached(&self) -> Option<Strokes> {
        Some(match self {
            PromptKey::Char(c) => vec![c.to_string().into_bytes()],
            PromptKey::Tab => vec![b"\t".to_vec()],
            PromptKey::BackTab => vec![b"\x1b[Z".to_vec()],
            PromptKey::Reply(_) => return None,
            PromptKey::Move(rows) | PromptKey::Tick(rows) => {
                let arrow: &[u8] = if *rows < 0 { b"\x1b[A" } else { b"\x1b[B" };
                vec![arrow.to_vec(); rows.unsigned_abs().into()]
            }
            PromptKey::Line(text) => vec![text.clone().into_bytes()],
            PromptKey::Enter => vec![ENTER.to_vec()],
        })
    }
}

pub fn entered(text: &str) -> Strokes {
    vec![text.as_bytes().to_vec(), ENTER.to_vec()]
}

/// DB에 두는 대기 한 건.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
pub struct HumanWait {
    pub id: WaitId,
    pub kind: WaitKind,
    /// `loom ask` 질문과 완료는 전문, 다른 대기는 한 줄이다. 없으면 빈 글이고, 화면은 종류 이름만 보인다.
    pub text: String,
    /// 대기가 시작된 유닉스 초.
    #[ts(type = "number")]
    pub since: i64,
    /// `loom ask` 질문이나 완료다. 화면에서 답할 수 있다(`Graph::answer`). 완료의 답은 이어서 맡기는 일이다. 다른 대기는 `prompt`의 버튼·입력칸이나 그 노드 터미널에서 답한다.
    pub answerable: bool,
    pub doc: Option<WaitDoc>,
    /// 완료(`Done`)만: 그 일을 맡긴 사람의 지시(`loom done --asked`). `text`는 결과(`loom done`의 요약)다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instruction: Option<String>,
    /// 완료(`Done`)만: 사람이 그 지시에 준 그림의 데몬 기계 경로(`loom done --image`). 화면은 `GET /api/waits/{id}/images/{index}`로 읽는다.
    #[serde(default)]
    pub images: Vec<String>,
    /// 신호(훅·서버·화면)의 대기만: 지금 터미널 화면에서 읽은 창이나 서버가 준 창. 읽지 못했으면 없다. 대기와 함께 DB에 둔다(`Graph::set_wait`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prompt: Option<Prompt>,
    /// `text`의 글 조각. 화면에 보낼 때만 채운다(`for_screen`). DB에 두지 않는다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<TextPart>,
    /// 질문(`Question`)만: 표준 꼴 선택지. `parts`와 같이 화면에 보낼 때만 채운다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
}

/// 화면에 보내는 대기: 저장한 대기와 데몬이 정한 표시 값.
#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScreenWait {
    #[serde(flatten)]
    #[ts(flatten)]
    pub wait: HumanWait,
    pub tone: WaitTone,
    /// 한 줄 미리 보기: 창이 있으면 `prompt.summary`, 없으면 `parts`의 글.
    pub summary: String,
}

/// 대기의 색과 아이콘, 알림 채널.
#[derive(Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WaitTone {
    Ask,
    Done,
}

/// 질문의 선택지 하나. `key`가 답으로 가는 글이다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Choice {
    pub key: char,
    pub label: String,
    pub recommended: bool,
}

/// 알림이 닫힌 방법.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Closed {
    /// 사람이 받은편지함에서 질문에 답했다.
    Answered {
        text: String,
    },
    /// 사람이 완료 줄에서 이어서 맡겼다.
    Instructed {
        text: String,
    },
    /// 사람이 완료를 [확인]했다.
    Confirmed,
    Unanswered {
        by: ClosedBy,
    },
    /// 알림이 열린 채 노드를 지웠다.
    NodeRemoved,
}

/// 「기록」 탭의 한 줄: 닫힌 질문이나 완료.
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClosedWait {
    /// 닫힌 차례. 다음 쪽은 이 값보다 작은 줄이다.
    pub seq: u32,
    pub wait: ScreenWait,
    /// 노드를 지웠으면 `id`가 없다.
    pub node: Who,
    #[ts(type = "number")]
    pub closed_at: i64,
    pub how: Closed,
    /// 답이 선택지 하나의 `key`와 같으면 그 `key`다.
    pub chosen: Option<char>,
}

/// 닫힌 알림 한 쪽. 새것부터.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClosedPage {
    pub waits: Vec<ClosedWait>,
    pub more: bool,
}

/// 사람이 화면에서 `loom ask` 질문에 쓴 답. `images`는 답에 붙인 그림의 데몬 기계 경로다(`POST /api/pastes`가 준 값, 목업 M30).
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AnswerWait {
    pub text: String,
    #[serde(default)]
    pub images: Vec<PathBuf>,
}

/// 사람이 알림에서 고른 승인 창 선택지. `prompt`는 화면이 보인 창 그대로, `option`은 그 `options`의 차례다.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChooseOption {
    pub prompt: Prompt,
    #[ts(type = "number")]
    pub option: usize,
}

/// 사람이 알림의 입력칸에 쓴 글. 데몬은 `prompt`가 지금 화면과 같을 때만 그 글과 Enter를 치고, 글을 저장하거나 로그에 쓰지 않는다.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TypeText {
    pub prompt: Prompt,
    pub text: String,
    /// 글로 답하는 선택지(`PromptOption::writes`)의 차례. 없으면 창의 입력칸(`Prompt::field`)이다.
    #[serde(default)]
    #[ts(optional, type = "number")]
    pub option: Option<usize>,
}
