use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::graph::ProcessState;
use crate::graph::waits::Choice;
use crate::ids::WaitId;

/// 문서 페이지(`/docs/{id}`)의 응답 막대 상태: 그 문서가 붙은 가장 최근 질문. 목업 docs/mockups/doc-answer.html.
#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum DocQuestion {
    None,
    Open {
        wait: WaitId,
        /// 질문 글의 표준 꼴 선택지. 문서에 결정 상자가 없으면 응답 막대가 이것을 고르게 한다. 상자가 있으면 막대는 상자만 보인다.
        #[serde(default)]
        choices: Vec<Choice>,
    },
    Answered {
        wait: WaitId,
        text: String,
        /// 답한 유닉스 초.
        #[ts(type = "number")]
        at: i64,
    },
    Closed {
        wait: WaitId,
    },
}

/// [문서] 탭의 한 줄(목업 docs/mockups/doc-reopen.html M13 ③). 사람은 닫힌 질문의 문서도 이 목록에서 다시 연다.
#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocRow {
    pub id: String,
    pub title: String,
    pub kind: DocGroup,
    /// 목업은 `docs/mockups`를 가진 폴더(워크트리·저장소) 이름, 보고서는 그 파일의 폴더 이름.
    pub folder: String,
    /// 마지막으로 붙인 때(사본의 수정 시각, 유닉스 초). 이 판 전에 붙인 문서는 사본이 없어 없다.
    #[ts(type = "number | null")]
    pub attached_at: Option<i64>,
    /// 원본 파일이 있다.
    pub source: bool,
    /// 붙일 때 남긴 사본이 있다.
    pub snapshot: bool,
}

#[derive(Serialize, Deserialize, TS, PartialEq, Eq, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DocGroup {
    Mockup,
    Report,
}

/// 문서 페이지의 대화 칸과 문서 판(목업 docs/mockups/live-doc.html M119 v2). 페이지가 짧은 간격으로 읽는다.
#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocTalk {
    /// 페이지의 말을 받는 노드. 없으면 대화 칸은 말을 보낼 수 없다.
    pub to: Option<TalkTo>,
    pub lines: Vec<TalkLine>,
    /// 페이지가 내는 문서 파일의 판(수정 시각과 크기). 바뀌면 페이지가 문서를 다시 읽는다.
    pub version: String,
}

#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TalkTo {
    pub name: String,
    pub process: ProcessState,
}

/// 대화 칸의 한 줄.
#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TalkLine {
    pub id: u32,
    pub said: Said,
    pub text: String,
    /// 유닉스 초.
    #[ts(type = "number")]
    pub at: i64,
}

#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(tag = "by", rename_all = "camelCase")]
#[ts(export)]
pub enum Said {
    /// 사람이 페이지에서 보낸 말. `answer`가 있으면 그 질문의 답으로 갔다. `received`는 받는 노드가 읽었다.
    Human {
        to: String,
        answer: Option<WaitId>,
        received: bool,
    },
    /// 에이전트가 `loom answer`로 페이지에 쓴 답.
    Agent { name: String },
}

/// 사람이 대화 칸에서 보낸 말(`POST /api/docs/{id}/talk`).
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PostTalk {
    pub text: String,
}
