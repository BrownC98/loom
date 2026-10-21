//! 사람 응답 대기: 노드가 사람의 답·승인을 기다린다는 표시 하나. 화면의 받은편지함·카드·사이드바가 이것만 읽는다.
//! 신호(에이전트의 `loom ask`, Claude 훅, Codex 스레드 상태, 터미널 화면)는 모두 `set_wait` 하나로 들어온다.
//! 지우는 길: 신호 쪽이 거두기(`set_wait(.., None)`. `loom ask` 대기는 에이전트의 `loom answered`), 사람의 키 입력(`typed`, `loom ask` 대기는 남긴다),
//! 받은편지함 [×]·노드 재시작(`clear_wait`), 프로세스 끝(`end_process`).
//! 근거: docs/design.md 「사람 응답 대기」.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Graph, NodeId, Refusal, parent_of, to_json};

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
}

/// 대기를 알린 신호. 신호는 자기가 건 대기만 거둔다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitSource {
    /// `loom ask`.
    Ask,
    /// Claude 훅.
    Hook,
    /// Codex 스레드 상태.
    Codex,
    /// 터미널 화면에서 찾은 확인 창(`screen`).
    Screen,
}

/// 화면에 보내는 대기 한 건.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HumanWait {
    pub kind: WaitKind,
    /// 한 줄. 에이전트·CLI가 쓴 글 그대로다. 없으면 빈 글이고, 화면은 종류 이름만 보인다.
    pub text: String,
    /// 대기가 시작된 유닉스 초.
    #[ts(type = "number")]
    pub since: i64,
}

/// DB `nodes.wait`의 JSON.
#[derive(Serialize, Deserialize)]
pub(super) struct Stored {
    #[serde(flatten)]
    pub wait: HumanWait,
    pub source: WaitSource,
}

/// 한 줄 글의 상한. 받은편지함 한 줄과 시스템 알림 한 줄이면 된다.
const TEXT_LIMIT_CHARS: usize = 200;

impl Graph {
    /// 신호 하나가 노드의 대기를 바꾼다. `None`이면 그 신호가 건 대기만 지운다.
    /// 같은 종류·글이면 그대로 둔다(시작 시각을 지킨다). 바뀌었을 때만 화면에 알린다.
    pub fn set_wait(
        &self,
        id: NodeId,
        source: WaitSource,
        wait: Option<(WaitKind, &str)>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let Some(now) = stored(&db, id)? else {
            return Ok(());
        };
        let next = match wait {
            Some((kind, text)) => {
                let text = one_line(text);
                if let Some(now) = &now
                    && now.wait.kind == kind
                    && now.wait.text == text
                {
                    return Ok(());
                }
                Some(Stored {
                    wait: HumanWait {
                        kind,
                        text,
                        since: unix_now(),
                    },
                    source,
                })
            }
            None if now.as_ref().is_some_and(|n| n.source == source) => None,
            None => return Ok(()),
        };
        db.execute(
            "UPDATE nodes SET wait = ?2 WHERE id = ?1",
            params![id.0, next.as_ref().map(to_json)],
        )?;
        self.publish_node(&db, id)
    }

    /// `loom ask`: 사람이 띄운 노드(부모 없음)만 사람에게 묻는다. 다른 노드는 부모에게 묻는다.
    pub fn ask(&self, id: NodeId, question: &str) -> Result<(), Refusal> {
        if question.trim().is_empty() {
            return Err(Refusal::EmptyQuestion);
        }
        if parent_of(&self.lock(), id)?.is_some() {
            return Err(Refusal::HasParent);
        }
        Ok(self.set_wait(id, WaitSource::Ask, Some((WaitKind::Question, question)))?)
    }

    /// 사람이 띄운 노드(부모 없음)이고 지금 사람 응답 대기가 없다. 턴 끝 질문 검사(`daemon::turn_end`)가 쓴다.
    /// 이번 턴에 `loom ask`나 Claude 질문 도구로 물었으면 대기가 남아 있다.
    pub fn unasked_root(&self, id: NodeId) -> Result<bool, Refusal> {
        let db = self.lock();
        if parent_of(&db, id)?.is_some() {
            return Ok(false);
        }
        Ok(stored(&db, id)?.is_some_and(|wait| wait.is_none()))
    }

    /// 아직 열린 `loom ask` 질문의 글. 사람 입력 훅이 에이전트에 알린다.
    /// 근거: 2026-10-21 사람 지시 "질문이 걸려있는 상태에서는 답변이 왔는지 확인하는 훅을 거는게 어때".
    pub fn open_question(&self, id: NodeId) -> Result<Option<String>, Refusal> {
        Ok(stored(&self.lock(), id)?
            .flatten()
            .filter(|now| now.source == WaitSource::Ask)
            .map(|now| now.wait.text))
    }

    /// 사람이 그 노드 터미널에 키를 눌렀다. `loom ask` 대기만 남기고 지운다. 그 입력이 답인지는 에이전트가 판단해 `loom answered`로 지운다.
    /// 근거: 2026-10-21 사람 지시 "사람이 정말 답변을 했을때만 알림이 제거되도록 알림제거는 사용자 입력을 에이전트가 판단하여 제거하도록 하라".
    pub fn typed(&self, id: NodeId) -> rusqlite::Result<()> {
        let asked = stored(&self.lock(), id)?
            .flatten()
            .is_some_and(|now| now.source == WaitSource::Ask);
        match asked {
            true => Ok(()),
            false => self.clear_wait(id),
        }
    }

    /// 사람이 받은편지함에서 지웠거나 노드 프로세스를 다시 띄웠다. 어느 신호의 대기든 지운다.
    pub fn clear_wait(&self, id: NodeId) -> rusqlite::Result<()> {
        let db = self.lock();
        let changed = db.execute(
            "UPDATE nodes SET wait = NULL WHERE id = ?1 AND wait IS NOT NULL",
            [id.0],
        )?;
        if changed > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }
}

/// 노드가 없으면 바깥 `None`, 대기가 없으면 안쪽 `None`.
fn stored(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<Option<Option<Stored>>> {
    let text: Option<Option<String>> = db
        .query_row("SELECT wait FROM nodes WHERE id = ?1", [id.0], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(text.map(|t| t.and_then(|t| serde_json::from_str(&t).ok())))
}

/// 비지 않은 첫 줄을 `TEXT_LIMIT_CHARS`자까지.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.chars().take(TEXT_LIMIT_CHARS).collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
