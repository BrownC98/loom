//! 사람 응답 대기: 노드가 사람의 답·승인을 기다린다는 표시. 화면의 받은편지함·카드·사이드바가 이것만 읽는다(`NodeView::waits`).
//! 두 가지다. 신호(Claude 훅, Codex 스레드 상태, 터미널 화면)의 대기는 노드마다 하나(`nodes.wait`)이고 `set_wait`로 들어온다.
//! `loom ask` 질문은 노드마다 여러 개(`questions` 표)다. 새 질문은 앞 질문을 지우지 않는다(2026-10-22 목업 M28 승인, grill-me처럼 한 번에 여러 질문).
//! `loom done --asked` 완료도 노드마다 여러 개(`dones` 표)다. 사람의 [확인](`dismiss`)과 이어서 맡기기(`answer`)만 지운다.
//! 지우는 길: 신호 쪽이 거두기(`set_wait(.., None)`), 에이전트의 `loom answered`(`answered`), 사람의 키 입력(`typed`, 질문은 남긴다),
//! 받은편지함 [×](`dismiss`), 노드 재시작(`relaunched`), 프로세스 끝(`end_process`).
//! 사람은 화면에서 질문에 바로 답할 수도 있다(`answer`). 답은 사람이 보낸 메시지로 노드에 가고 그 질문은 그때 지운다.
//! 질문·완료가 걸리고 닫힐 때마다 같은 트랜잭션에서 이벤트 기록에 한 줄을 남긴다(`events::Entry::Asked` 등). 신호의 대기는 남기지 않는다.
//! 근거: docs/design.md 「사람 응답 대기」.

use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{self, ClosedBy, Entry};
use super::mailbox;
use crate::choices::{Choice, choices, offers_choices_without_tail};
use crate::link_text::{TextPart, bare_link, parts};
use crate::profile::AgentId;
use crate::protocol::{MessageKind, Question};

use super::{Graph, NodeId, NodeState, Refusal, from_json, parent_of, state_of, to_json};

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

/// 노드마다 하나인 대기를 알린 신호. 신호는 자기가 건 대기만 거둔다. `loom ask` 질문은 신호가 아니다(`questions` 표).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitSource {
    /// Claude 훅.
    Hook,
    /// Codex 스레드 상태.
    Codex,
    /// 터미널 화면에서 찾은 확인 창(`screen`).
    Screen,
}

/// 대기 번호. 사람은 `Q12`로 읽고 쓴다. 데몬 전체에서 늘기만 하고(DB `wait_ids`) 다시 쓰지 않는다.
/// 근거: 2026-10-21 사람 지시 "질문에 id 부여하고 복사하기 편하게 만들어서".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitId(pub u32);

impl fmt::Display for WaitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Q{}", self.0)
    }
}

/// 질문·완료에 붙은 문서(user-doc 목업·보고서 HTML). 화면은 ID와 제목만 받고 `/api/docs/{id}/`로 연다. 파일 경로는 데몬의 문서 ID 표(`user_doc::doc_file`)만 안다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitDoc {
    /// `M25`, `R3` 같은 문서 ID.
    pub id: String,
    pub title: String,
}

/// 터미널 승인 창의 본문과 선택지(`screen::prompt`). 화면은 선택지마다 버튼을 그리고, 누르면 이 값을 그대로 돌려보낸다(`Graph::approval`).
/// 데몬은 보내기 직전 화면을 다시 읽어 같은 값일 때만 키를 친다. 그래서 다른 창에 키가 들어가지 않는다(2026-10-23 목업 M11 승인).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Prompt {
    /// 창 위부터 마지막 선택지까지 화면 줄 그대로. 같은 창인지는 이 줄들이 정한다. `body`는 보기 좋게 다듬은 글이라 다른 명령이 같은 글이 될 수 있다(2026-10-23 리뷰: 명령 줄 「Tip:; touch one」).
    pub rows: Vec<String>,
    /// 창 본문(도구, 명령, 이유) 줄들. 명령 줄은 앞 공백까지 터미널 글 그대로다.
    pub body: Vec<String>,
    /// 무엇을 승인하는지 한 줄(명령). 알약 미리 보기가 쓴다.
    pub summary: String,
    pub options: Vec<PromptOption>,
}

/// 승인 창의 선택지 하나. `label`은 터미널 글(영어) 그대로다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptOption {
    pub label: String,
    pub key: PromptKey,
}

/// 선택지를 고르는 키 하나. 누르면 바로 정한다(Claude 2.1.309, Codex 0.159.2 실측: 숫자 키 하나, Enter 없음).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum PromptKey {
    Char(char),
    Tab,
    BackTab,
}

impl PromptKey {
    /// 터미널에 쓰는 바이트.
    pub fn bytes(self) -> Vec<u8> {
        match self {
            PromptKey::Char(c) => c.to_string().into_bytes(),
            PromptKey::Tab => b"\t".to_vec(),
            PromptKey::BackTab => b"\x1b[Z".to_vec(),
        }
    }
}

/// 화면에 보내는 대기 한 건.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HumanWait {
    pub id: WaitId,
    pub kind: WaitKind,
    /// `loom ask` 질문과 완료는 전문, 다른 대기는 한 줄이다. 없으면 빈 글이고, 화면은 종류 이름만 보인다.
    pub text: String,
    /// 대기가 시작된 유닉스 초.
    #[ts(type = "number")]
    pub since: i64,
    /// `loom ask` 질문이나 완료다. 화면에서 답할 수 있다(`Graph::answer`). 완료의 답은 이어서 맡기는 일이다. 다른 대기는 그 노드 터미널에서만 답한다.
    pub answerable: bool,
    pub doc: Option<WaitDoc>,
    /// 완료(`Done`)만: 그 일을 맡긴 사람의 지시(`loom done --asked`). `text`는 결과(`loom done`의 요약)다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instruction: Option<String>,
    /// 완료(`Done`)만: 사람이 그 지시에 준 그림의 데몬 기계 경로(`loom done --image`). 화면은 `GET /api/waits/{id}/images/{index}`로 읽는다.
    #[serde(default)]
    pub images: Vec<String>,
    /// 승인(`Approval`) 신호 대기만: 지금 터미널 화면에서 읽은 승인 창. 읽지 못했으면 없다. DB에 두지 않는다(`Graph::set_prompt`).
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

impl HumanWait {
    /// 화면에 보내는 대기: 글을 데몬이 나눈 조각과 선택지를 채운다. 화면은 글을 읽어 링크·선택지를 찾지 않는다.
    pub(super) fn for_screen(self) -> HumanWait {
        HumanWait {
            parts: parts(&self.text),
            choices: match self.kind {
                WaitKind::Question => choices(&self.text),
                WaitKind::Approval
                | WaitKind::Trust
                | WaitKind::Auth
                | WaitKind::Input
                | WaitKind::Done => Vec::new(),
            },
            ..self
        }
    }
}

/// DB `nodes.wait`의 JSON.
#[derive(Serialize, Deserialize)]
pub(super) struct Stored {
    #[serde(flatten)]
    pub wait: HumanWait,
    pub source: WaitSource,
}

/// 열린 `loom ask` 질문. 사람 입력 훅이 에이전트에 알린다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: WaitId,
    pub text: String,
}

/// 다른 대기의 한 줄 글 상한. 받은편지함 한 줄과 시스템 알림 한 줄이면 된다.
const TEXT_LIMIT_CHARS: usize = 200;
/// `loom ask` 질문 전문과 완료의 지시(`loom done --asked`)의 상한. 질문은 화면에서 다 보이고(2026-10-21 사람 지시 "질문 전문을 보기 편하게"), 답 메시지에 다시 담는다.
/// 답 메시지 본문(`BRIEF_LIMIT_BYTES` 4KB) 안에 한국어 질문(3바이트/자)과 답이 함께 들어가는 길이다.
const QUESTION_LIMIT_CHARS: usize = 1000;
/// 화면에서 보낸 답 메시지의 보낸 이. 노드가 아니므로 `messages.sender`는 비운다.
pub const HUMAN: &str = "human";

impl Graph {
    /// 신호 하나가 노드의 대기를 바꾼다. `None`이면 그 신호가 건 대기만 지운다.
    /// 같은 종류·글이면 그대로 둔다(시작 시각을 지킨다). 바뀌었을 때만 화면에 알린다.
    /// `token`은 신호를 낸 프로세스의 토큰이다. 다시 띄운 노드의 옛 프로세스·옛 화면·옛 서버가 늦게 낸 신호는 대기를 다시 열지 못한다(`relaunched`가 토큰을 바꾼다).
    /// 근거: 2026-10-23 리뷰 「다시 띄운 뒤 옛 승인」: 옛 대기가 새 PTY에 키를 쳤다.
    pub fn set_wait(
        &self,
        token: &str,
        source: WaitSource,
        wait: Option<(WaitKind, &str)>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let Some(id) = super::nodes::caller_in(&db, token)? else {
            return Ok(());
        };
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
                    wait: new_wait(&db, kind, text, false, None)?,
                    source,
                })
            }
            None if now.as_ref().is_some_and(|n| n.source == source) => None,
            None => return Ok(()),
        };
        put(&db, id, next.as_ref())?;
        self.publish_node(&db, id)
    }

    /// 부모 없는 노드의 `loom ask`: 사람에게 묻는다. 부모가 있는 노드는 부모에게 묻는다(`ask_parent`). 데몬이 고른다.
    /// 물을 때마다 새 번호의 질문을 더한다. 앞 질문은 열린 채로 남는다. 같은 질문을 다시 물어도 새 질문이다.
    pub fn ask(&self, id: NodeId, question: &str, doc: Option<WaitDoc>) -> Result<WaitId, Refusal> {
        let question = question.trim();
        if question.is_empty() {
            return Err(Refusal::EmptyQuestion);
        }
        // 화면이 보이는 글(잘린 글)을 잠금 밖에서 검사한다.
        let text: String = question.chars().take(QUESTION_LIMIT_CHARS).collect();
        let malformed = bare_link(&text)
            .map(Refusal::BareLink)
            .or_else(|| offers_choices_without_tail(&text).then_some(Refusal::BadChoices));
        let mut db = self.lock();
        if let Some(refusal) = malformed {
            return Err(refusal);
        }
        let tx = db.transaction()?;
        let wait = new_wait(&tx, WaitKind::Question, text.clone(), true, doc.clone())?;
        let asked = wait.id;
        tx.execute(
            "INSERT INTO questions (id, node, wait) VALUES (?1, ?2, ?3)",
            params![asked.0, id.0, to_json(&wait)],
        )?;
        let entry = Entry::Asked {
            node: events::who(&tx, id)?,
            wait: asked,
            text,
            doc,
        };
        events::write(&tx, &entry, None)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        Ok(asked)
    }

    /// 사람이 화면에서 `loom ask` 질문이나 완료에 답했다. 답은 사람이 보낸 큐 메시지로 그 노드에 가고, 그 질문이나 완료만 바로 지운다. 답을 받은 노드를 돌려준다.
    /// 에이전트가 답인지 판단할 일이 없다. 근거: 2026-10-21 사람 지시 "질문마다 답변 입력창을 두는 방안".
    /// `images`는 답에 붙인 그림 파일이다. 메시지에 작은따옴표로 묶은 절대 경로를 한 줄로 붙인다. 에이전트는 경로로 그림을 읽는다.
    /// 그림만 보내도 답이다(2026-10-22 목업 M30 승인, 사람 지시 "답변 입력창에 이미지 입력도 가능하게 해줘").
    pub fn answer(&self, wait: WaitId, text: &str, images: &[PathBuf]) -> Result<NodeId, Refusal> {
        let text = text.trim();
        if text.is_empty() && images.is_empty() {
            return Err(Refusal::EmptyAnswer);
        }
        let mut db = self.lock();
        let count = images.len() as u32;
        let images = match images {
            [] => String::new(),
            paths => {
                let quoted: Vec<String> =
                    paths.iter().map(|p| quote(&p.to_string_lossy())).collect();
                format!("\nImages: {}", quoted.join(" "))
            }
        };
        type Record = fn(events::Who, WaitId, String, u32) -> Entry;
        let (node, what, body, record): (NodeId, MessageKind, String, Record) = match open_ask(
            &db, wait,
        ) {
            Ok((node, question)) => (
                node,
                MessageKind::Answer {
                    question: Question::Inbox { id: wait.0 },
                },
                format!(
                    "Answer to loom ask question {wait} ({}): {text}{images}\nThe human answered from the loom inbox. The question is closed: do not call loom answered.",
                    question.text
                ),
                |node, wait, text, images| Entry::Answered {
                    node,
                    wait,
                    text,
                    images,
                },
            ),
            // 완료 줄의 입력창: 같은 노드에 이어서 맡기는 일이다(2026-10-22 사람 지시 "완료알림에는 추가 지시를 받을 수 있도록 입력창도 추가해").
            Err(Refusal::NoWait(_)) => (
                done_node(&db, wait)?,
                MessageKind::Note,
                format!(
                    "New instruction from the human in the loom inbox, after your done report: {text}{images}"
                ),
                |node, wait, text, images| Entry::Instructed {
                    node,
                    wait,
                    text,
                    images,
                },
            ),
            Err(e) => return Err(e),
        };
        let tx = db.transaction()?;
        if matches!(what, MessageKind::Answer { .. }) {
            tx.execute("INSERT OR IGNORE INTO answers (id) VALUES (?1)", [wait.0])?;
        }
        mailbox::enqueue_human(&tx, node, &what, &body)?;
        tx.execute("DELETE FROM questions WHERE id = ?1", [wait.0])?;
        tx.execute("DELETE FROM dones WHERE id = ?1", [wait.0])?;
        let entry = record(events::who(&tx, node)?, wait, text.to_owned(), count);
        events::write(&tx, &entry, None)?;
        tx.commit()?;
        self.publish_node(&db, node)?;
        self.mail.notify_waiters();
        Ok(node)
    }

    /// `loom answered`: 사람의 입력이 이 노드의 질문 `ids`에 답했다고 에이전트가 판단했다. 그 질문을 지우고 지운 번호를 돌려준다.
    /// 번호가 없으면 열린 질문이 하나일 때만 그것을 지운다. 여럿이면 거절하고 열린 번호를 알린다(2026-10-22 목업 M28 승인: 모두 닫지 않는다).
    pub fn answered(&self, id: NodeId, ids: &[WaitId]) -> Result<Vec<WaitId>, Refusal> {
        let mut db = self.lock();
        let open: Vec<WaitId> = questions(&db, id)?.iter().map(|q| q.id).collect();
        let closing = match ids {
            [] if open.len() > 1 => return Err(Refusal::SeveralOpen(open)),
            [] => open,
            ids => {
                if let Some(missing) = ids.iter().find(|q| !open.contains(q)) {
                    return Err(Refusal::NoWait(*missing));
                }
                ids.to_vec()
            }
        };
        let tx = db.transaction()?;
        for q in &closing {
            tx.execute("DELETE FROM questions WHERE id = ?1", [q.0])?;
            unanswered(&tx, id, *q, ClosedBy::Agent)?;
        }
        tx.commit()?;
        if !closing.is_empty() {
            self.publish_node(&db, id)?;
        }
        Ok(closing)
    }

    /// 사람이 띄운 노드(부모 없음)이고, 지금 사람 응답 대기가 하나도 없고, 마지막 입력 뒤 `loom done`을 부르지 않았다.
    /// 턴 끝 검사(`daemon::turn_end`)가 쓴다. 이번 턴에 `loom ask`나 Claude 질문 도구로 물었으면 대기가 남아 있다.
    pub fn unreported_root(&self, id: NodeId) -> Result<bool, Refusal> {
        let db = self.lock();
        if parent_of(&db, id)?.is_some() || state_of(&db, id)? != NodeState::Running {
            return Ok(false);
        }
        Ok(stored(&db, id)?.is_some_and(|wait| wait.is_none()) && questions(&db, id)?.is_empty())
    }

    /// 열린 `loom ask` 질문들, 물은 차례대로. 사람 입력 훅이 에이전트에 알리고, `loom ask`가 번호를 찍는다.
    /// 근거: 2026-10-21 사람 지시 "질문이 걸려있는 상태에서는 답변이 왔는지 확인하는 훅을 거는게 어때".
    pub fn answered_in_inbox(&self, wait: WaitId) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS (SELECT 1 FROM answers WHERE id = ?1)",
            [wait.0],
            |row| row.get(0),
        )
    }

    pub fn open_questions(&self, id: NodeId) -> Result<Vec<OpenQuestion>, Refusal> {
        Ok(questions(&self.lock(), id)?
            .into_iter()
            .map(|q| OpenQuestion {
                id: q.id,
                text: q.text,
            })
            .collect())
    }

    /// 사람이 그 노드 터미널에 키를 눌렀다. 신호의 대기만 지운다. 그 입력이 질문의 답인지는 에이전트가 판단해 `loom answered`로 지운다.
    /// 근거: 2026-10-21 사람 지시 "사람이 정말 답변을 했을때만 알림이 제거되도록 알림제거는 사용자 입력을 에이전트가 판단하여 제거하도록 하라".
    pub fn typed(&self, id: NodeId) -> rusqlite::Result<()> {
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

    /// 사람이 받은편지함 줄의 [×]를 눌렀다. 그 번호의 대기 하나(질문이든 신호의 대기든)를 지운다.
    /// `loom ask` 질문이면 그 노드에 짧은 큐 메시지로 알린다. 에이전트가 오지 않을 답을 기다리지 않게 한다(2026-10-22 사람 지시 "Q257은 답 없이 닫혔다. 라고만 보내라").
    pub fn dismiss(&self, wait: WaitId) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let node: Option<u32> = tx
            .query_row(
                "DELETE FROM questions WHERE id = ?1 RETURNING node",
                [wait.0],
                |row| row.get(0),
            )
            .optional()?;
        let asked = node.is_some();
        let node = match node.map(NodeId) {
            Some(node) => {
                mailbox::enqueue_human(
                    &tx,
                    node,
                    &MessageKind::Note,
                    &format!("{wait} was closed without an answer."),
                )?;
                unanswered(&tx, node, wait, ClosedBy::Human)?;
                node
            }
            None => match done_node(&tx, wait) {
                Ok(node) => {
                    tx.execute("DELETE FROM dones WHERE id = ?1", [wait.0])?;
                    let entry = Entry::Confirmed {
                        node: events::who(&tx, node)?,
                        wait,
                    };
                    events::write(&tx, &entry, None)?;
                    node
                }
                Err(Refusal::NoWait(_)) => tx
                    .query_row(
                        "UPDATE nodes SET wait = NULL WHERE wait ->> 'id' = ?1 RETURNING id",
                        [wait.0],
                        |row| row.get(0),
                    )
                    .optional()?
                    .map(NodeId)
                    .ok_or(Refusal::NoWait(wait))?,
                Err(e) => return Err(e),
            },
        };
        tx.commit()?;
        if asked {
            self.mail.notify_waiters();
        }
        Ok(self.publish_node(&db, node)?)
    }

    /// 받은편지함 완료 줄의 `index`번째 그림 파일. 그 완료가 지워졌으면 거절한다.
    pub fn done_image(&self, wait: WaitId, index: usize) -> Result<PathBuf, Refusal> {
        let done: Option<String> = self
            .lock()
            .query_row("SELECT wait FROM dones WHERE id = ?1", [wait.0], |row| {
                row.get(0)
            })
            .optional()?;
        done.and_then(|d| serde_json::from_str::<HumanWait>(&d).ok())
            .and_then(|d| d.images.into_iter().nth(index))
            .map(PathBuf::from)
            .ok_or(Refusal::NoWait(wait))
    }

    /// 사람이 알림에서 승인 창의 선택지를 눌렀다. 그 번호가 열린 승인 신호 대기면 그 노드와 에이전트를 돌려준다. 키는 데몬이 화면을 다시 읽고 친다.
    pub fn approval(&self, wait: WaitId) -> Result<(NodeId, Option<AgentId>), Refusal> {
        let found: Option<(u32, Option<AgentId>)> = self
            .lock()
            .query_row(
                "SELECT id, agent FROM nodes WHERE wait ->> 'id' = ?1 AND wait ->> 'kind' = 'approval'",
                [wait.0],
                |row| Ok((row.get(0)?, from_json(row, 1)?)),
            )
            .optional()?;
        let (node, agent) = found.ok_or(Refusal::NoWait(wait))?;
        Ok((NodeId(node), agent))
    }
}

/// 사람이 띄운 노드의 `loom done`: 받은편지함에 완료를 더한다. 확인하지 않은 앞 완료는 남는다.
/// 에이전트가 직접 알린다. 턴 끝에서 짐작하지 않는다(2026-10-22 사람 지시 "완료 알림 방식도 질문처럼 표준화 해라").
pub(super) fn put_done(
    db: &rusqlite::Connection,
    id: NodeId,
    asked: &str,
    summary: &str,
    images: &[PathBuf],
    doc: Option<WaitDoc>,
) -> rusqlite::Result<()> {
    let summary = summary.trim().to_owned();
    let instruction: String = asked.chars().take(QUESTION_LIMIT_CHARS).collect();
    let mut wait = new_wait(db, WaitKind::Done, summary.clone(), true, doc.clone())?;
    wait.instruction = Some(instruction.clone());
    wait.images = images
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let done = wait.id;
    db.execute(
        "INSERT INTO dones (id, node, wait) VALUES (?1, ?2, ?3)",
        params![done.0, id.0, to_json(&wait)],
    )?;
    let entry = Entry::Reported {
        node: events::who(db, id)?,
        wait: done,
        summary,
        instruction,
        doc,
    };
    events::write(db, &entry, None)
}

/// 노드의 열린 질문을 모두 답 없이 닫는다. 닫은 수를 돌려준다.
pub(super) fn drop_questions(
    db: &rusqlite::Connection,
    id: NodeId,
    by: ClosedBy,
) -> rusqlite::Result<usize> {
    let closed = db
        .prepare("DELETE FROM questions WHERE node = ?1 RETURNING id")?
        .query_map([id.0], |row| row.get(0).map(WaitId))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for wait in &closed {
        unanswered(db, id, *wait, by)?;
    }
    Ok(closed.len())
}

fn unanswered(
    db: &rusqlite::Connection,
    id: NodeId,
    wait: WaitId,
    by: ClosedBy,
) -> rusqlite::Result<()> {
    let entry = Entry::Unanswered {
        node: events::who(db, id)?,
        wait,
        by,
    };
    events::write(db, &entry, None)
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

/// 새 번호를 받은 대기. `wait_ids`는 AUTOINCREMENT라 지운 번호를 다시 주지 않는다. 받은 줄은 바로 지워 표를 비워 둔다.
fn new_wait(
    db: &rusqlite::Connection,
    kind: WaitKind,
    text: String,
    answerable: bool,
    doc: Option<WaitDoc>,
) -> rusqlite::Result<HumanWait> {
    db.execute("INSERT INTO wait_ids DEFAULT VALUES", [])?;
    let id = db.last_insert_rowid();
    db.execute("DELETE FROM wait_ids WHERE id = ?1", [id])?;
    Ok(HumanWait {
        id: WaitId(id as u32),
        kind,
        text,
        since: unix_now(),
        answerable,
        doc,
        instruction: None,
        images: Vec::new(),
        prompt: None,
        parts: Vec::new(),
        choices: Vec::new(),
    })
}

fn put(db: &rusqlite::Connection, id: NodeId, wait: Option<&Stored>) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET wait = ?2 WHERE id = ?1",
        params![id.0, wait.map(to_json)],
    )?;
    Ok(())
}

/// 번호가 `wait`인 열린 `loom ask` 질문과 그 노드. 지웠거나 신호의 대기면 거절한다.
fn open_ask(db: &rusqlite::Connection, wait: WaitId) -> Result<(NodeId, HumanWait), Refusal> {
    let found: Option<(u32, String)> = db
        .query_row(
            "SELECT node, wait FROM questions WHERE id = ?1",
            [wait.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    found
        .and_then(|(node, text)| Some((NodeId(node), serde_json::from_str(&text).ok()?)))
        .ok_or(Refusal::NoWait(wait))
}

/// 노드의 열린 질문, 물은 차례(번호 차례)대로. 읽지 못하는 줄(앞으로 바뀐 형식)은 건너뛴다.
fn questions(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<Vec<HumanWait>> {
    let mut stmt = db.prepare_cached("SELECT wait FROM questions WHERE node = ?1 ORDER BY id")?;
    let rows = stmt
        .query_map([id.0], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .iter()
        .filter_map(|t| serde_json::from_str(t).ok())
        .collect())
}

/// 번호가 `wait`인 확인하지 않은 완료의 노드.
fn done_node(db: &rusqlite::Connection, wait: WaitId) -> Result<NodeId, Refusal> {
    db.query_row("SELECT node FROM dones WHERE id = ?1", [wait.0], |row| {
        row.get(0)
    })
    .optional()?
    .map(NodeId)
    .ok_or(Refusal::NoWait(wait))
}

/// 셸처럼 작은따옴표로 묶는다. 빈칸·한글 이름도 경로 하나로 읽힌다(화면 터미널 붙여넣기 `quotePath`와 같은 꼴).
fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
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
