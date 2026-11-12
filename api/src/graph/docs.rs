use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
