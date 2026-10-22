//! 사람 응답 대기: 노드가 사람의 답·승인을 기다린다는 표시. 화면의 받은편지함·카드·사이드바가 이것만 읽는다(`NodeView::waits`).
//! 두 가지다. 신호(Claude 훅, Codex 스레드 상태, 터미널 화면)의 대기는 노드마다 하나(`nodes.wait`)이고 `set_wait`로 들어온다.
//! `loom ask` 질문은 노드마다 여러 개(`questions` 표)다. 새 질문은 앞 질문을 지우지 않는다(2026-10-22 목업 M28 승인, grill-me처럼 한 번에 여러 질문).
//! 지우는 길: 신호 쪽이 거두기(`set_wait(.., None)`), 에이전트의 `loom answered`(`answered`), 사람의 키 입력(`typed`, 질문은 남긴다),
//! 받은편지함 [×](`dismiss`), 노드 재시작(`clear_wait`), 프로세스 끝(`end_process`).
//! 사람은 화면에서 질문에 바로 답할 수도 있다(`answer`). 답은 사람이 보낸 메시지로 노드에 가고 그 질문은 그때 지운다.
//! 근거: docs/design.md 「사람 응답 대기」.

use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::mailbox;
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

/// 질문에 붙은 문서(user-doc 목업·보고서). 화면은 ID와 제목만 받는다. 파일 경로는 데몬만 안다(`Stored::doc_path`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitDoc {
    /// `M25`, `R3` 같은 문서 ID.
    pub id: String,
    pub title: String,
}

/// `loom ask --doc`이 붙인 문서와 그 파일.
pub struct Attached {
    pub doc: WaitDoc,
    pub path: PathBuf,
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
}

/// DB `nodes.wait`의 JSON.
#[derive(Serialize, Deserialize)]
pub(super) struct Stored {
    #[serde(flatten)]
    pub wait: HumanWait,
    pub source: WaitSource,
}

/// DB `questions.wait`의 JSON: 열린 `loom ask` 질문 하나.
#[derive(Serialize, Deserialize)]
pub(super) struct Asked {
    #[serde(flatten)]
    pub wait: HumanWait,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_path: Option<PathBuf>,
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

    /// `loom ask`: 사람이 띄운 노드(부모 없음)만 사람에게 묻는다. 다른 노드는 부모에게 묻는다.
    /// 물을 때마다 새 번호의 질문을 더한다. 앞 질문은 열린 채로 남는다. 같은 질문을 다시 물어도 새 질문이다.
    pub fn ask(
        &self,
        id: NodeId,
        question: &str,
        attached: Option<Attached>,
    ) -> Result<WaitId, Refusal> {
        let question = question.trim();
        if question.is_empty() {
            return Err(Refusal::EmptyQuestion);
        }
        let db = self.lock();
        if parent_of(&db, id)?.is_some() {
            return Err(Refusal::HasParent);
        }
        let text = question.chars().take(QUESTION_LIMIT_CHARS).collect();
        let (doc, doc_path) = attached.map(|a| (a.doc, a.path)).unzip();
        let wait = new_wait(&db, WaitKind::Question, text, true, doc)?;
        let asked = wait.id;
        db.execute(
            "INSERT INTO questions (id, node, wait) VALUES (?1, ?2, ?3)",
            params![asked.0, id.0, to_json(&Asked { wait, doc_path })],
        )?;
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
        let images = match images {
            [] => String::new(),
            paths => {
                let quoted: Vec<String> =
                    paths.iter().map(|p| quote(&p.to_string_lossy())).collect();
                format!("\nImages: {}", quoted.join(" "))
            }
        };
        let (node, body) = match open_ask(&db, wait) {
            Ok((node, question)) => (
                node,
                format!(
                    "Answer to loom ask question {wait} ({}): {text}{images}\nThe human answered from the loom inbox. The question is closed: do not call loom answered.",
                    question.wait.text
                ),
            ),
            // 완료 줄의 입력창: 같은 노드에 이어서 맡기는 일이다(2026-10-22 사람 지시 "완료알림에는 추가 지시를 받을 수 있도록 입력창도 추가해").
            Err(Refusal::NoWait(_)) => (
                done_node(&db, wait)?,
                format!(
                    "New instruction from the human in the loom inbox, after your done report: {text}{images}"
                ),
            ),
            Err(e) => return Err(e),
        };
        let tx = db.transaction()?;
        mailbox::enqueue_human(&tx, node, &body)?;
        tx.execute("DELETE FROM questions WHERE id = ?1", [wait.0])?;
        tx.execute(
            "UPDATE nodes SET done = NULL WHERE done ->> 'id' = ?1",
            [wait.0],
        )?;
        tx.commit()?;
        self.publish_node(&db, node)?;
        self.mail.notify_waiters();
        Ok(node)
    }

    /// `loom answered`: 사람의 입력이 이 노드의 질문 `ids`에 답했다고 에이전트가 판단했다. 그 질문을 지우고 지운 번호를 돌려준다.
    /// 번호가 없으면 열린 질문이 하나일 때만 그것을 지운다. 여럿이면 거절하고 열린 번호를 알린다(2026-10-22 목업 M28 승인: 모두 닫지 않는다).
    pub fn answered(&self, id: NodeId, ids: &[WaitId]) -> Result<Vec<WaitId>, Refusal> {
        let db = self.lock();
        let open: Vec<WaitId> = questions(&db, id)?.iter().map(|q| q.wait.id).collect();
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
        for q in &closing {
            db.execute("DELETE FROM questions WHERE id = ?1", [q.0])?;
        }
        if !closing.is_empty() {
            self.publish_node(&db, id)?;
        }
        Ok(closing)
    }

    /// 열린 `loom ask` 질문에 붙은 문서 파일. 사람이 화면의 문서 링크를 눌렀다. 열린 질문의 문서만 연다.
    pub fn wait_doc(&self, wait: WaitId) -> Result<PathBuf, Refusal> {
        open_ask(&self.lock(), wait)?
            .1
            .doc_path
            .ok_or(Refusal::NoWait(wait))
    }

    /// 사람이 띄운 노드(부모 없음)이고 지금 사람 응답 대기가 하나도 없다. 턴 끝 질문 검사(`daemon::turn_end`)가 쓴다.
    /// 이번 턴에 `loom ask`나 Claude 질문 도구로 물었으면 대기가 남아 있다.
    pub fn unasked_root(&self, id: NodeId) -> Result<bool, Refusal> {
        let db = self.lock();
        if parent_of(&db, id)?.is_some() {
            return Ok(false);
        }
        Ok(stored(&db, id)?.is_some_and(|wait| wait.is_none()) && questions(&db, id)?.is_empty())
    }

    /// 열린 `loom ask` 질문들, 물은 차례대로. 사람 입력 훅이 에이전트에 알리고, `loom ask`가 번호를 찍는다.
    /// 근거: 2026-10-21 사람 지시 "질문이 걸려있는 상태에서는 답변이 왔는지 확인하는 훅을 거는게 어때".
    pub fn open_questions(&self, id: NodeId) -> Result<Vec<OpenQuestion>, Refusal> {
        Ok(questions(&self.lock(), id)?
            .into_iter()
            .map(|q| OpenQuestion {
                id: q.wait.id,
                text: q.wait.text,
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
        if let Some(node) = node {
            mailbox::enqueue_human(
                &tx,
                NodeId(node),
                &format!("{wait} was closed without an answer."),
            )?;
        }
        tx.commit()?;
        if node.is_some() {
            self.mail.notify_waiters();
        }
        let node = match node {
            Some(node) => node,
            None => db
                .query_row(
                    "UPDATE nodes SET
                        wait = CASE WHEN wait ->> 'id' = ?1 THEN NULL ELSE wait END,
                        done = CASE WHEN done ->> 'id' = ?1 THEN NULL ELSE done END
                     WHERE wait ->> 'id' = ?1 OR done ->> 'id' = ?1 RETURNING id",
                    [wait.0],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(Refusal::NoWait(wait))?,
        };
        Ok(self.publish_node(&db, NodeId(node))?)
    }

    /// 받은편지함 완료 줄의 `index`번째 그림 파일. 그 완료가 지워졌으면 거절한다.
    pub fn done_image(&self, wait: WaitId, index: usize) -> Result<PathBuf, Refusal> {
        let done: Option<String> = self
            .lock()
            .query_row(
                "SELECT done FROM nodes WHERE done ->> 'id' = ?1",
                [wait.0],
                |row| row.get(0),
            )
            .optional()?;
        done.and_then(|d| serde_json::from_str::<HumanWait>(&d).ok())
            .and_then(|d| d.images.into_iter().nth(index))
            .map(PathBuf::from)
            .ok_or(Refusal::NoWait(wait))
    }

    /// 노드 프로세스를 다시 띄웠다. 신호의 대기와 열린 질문을 모두 지운다.
    pub fn clear_wait(&self, id: NodeId) -> rusqlite::Result<()> {
        let db = self.lock();
        let changed = db.execute(
            "UPDATE nodes SET wait = NULL WHERE id = ?1 AND wait IS NOT NULL",
            [id.0],
        )? + db.execute("DELETE FROM questions WHERE node = ?1", [id.0])?;
        if changed > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }
}

/// 사람이 띄운 노드의 `loom done`: 받은편지함에 완료를 건다. 자리(`nodes.done`)는 노드마다 하나다. 확인하지 않은 앞 완료는 이번 완료로 바꾼다.
/// 에이전트가 직접 알린다. 턴 끝에서 짐작하지 않는다(2026-10-22 사람 지시 "완료 알림 방식도 질문처럼 표준화 해라").
pub(super) fn put_done(
    db: &rusqlite::Connection,
    id: NodeId,
    asked: &str,
    summary: &str,
    images: &[PathBuf],
) -> rusqlite::Result<()> {
    let mut wait = new_wait(db, WaitKind::Done, summary.trim().to_owned(), true, None)?;
    wait.instruction = Some(asked.chars().take(QUESTION_LIMIT_CHARS).collect());
    wait.images = images
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    db.execute(
        "UPDATE nodes SET done = ?2 WHERE id = ?1",
        params![id.0, to_json(&wait)],
    )?;
    Ok(())
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
fn open_ask(db: &rusqlite::Connection, wait: WaitId) -> Result<(NodeId, Asked), Refusal> {
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
fn questions(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<Vec<Asked>> {
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
    db.query_row(
        "SELECT id FROM nodes WHERE done ->> 'id' = ?1",
        [wait.0],
        |row| row.get(0),
    )
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
