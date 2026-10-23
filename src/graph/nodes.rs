//! 노드를 더하고 지우고, 이름과 부모를 바꾸고(입양), 프로세스가 끝난 노드를 고친다.
//! 근거: docs/design.md 「노드 제거와 입양」.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::events::{self, ClosedBy, Entry};
use super::waits::Prompt;
use super::{
    Db, Graph, GraphEvent, Kind, Model, NodeId, NodeState, Place, Refusal, Toward, TreeLimits,
    ambiguous, from_json, mailbox, name_of, parent_of, place_label, place_of, set_state, state_of,
    to_json, waits,
};
use crate::accounts::AccountId;
use crate::profile::{AgentId, BRIEF_LIMIT_BYTES, ServerLink};

/// `Graph::add_child`가 더할 자식.
pub struct NewChild<'a> {
    pub name: &'a str,
    /// 없으면 부모와 같은 워크트리에 든다. 저장소는 늘 부모와 같다.
    pub worktree: Option<&'a str>,
    pub agent: Option<AgentId>,
    pub brief: &'a str,
    /// `loom spawn --role`의 역할 이름. 관문을 고를 때 쓴다(`protocol::COORDINATOR_ROLE`, `CI_ROLE`).
    pub role: Option<&'a str>,
}

/// 에이전트 없이 띄운 노드(셸) 이름의 앞부분.
const SHELL_NAME: &str = "shell";

/// 띄운 노드와 함께 남기는 값. 데몬이 다시 시작해도 노드에 다시 붙게 한다.
pub struct Launched {
    /// 노드가 데몬을 부를 때 내는 토큰(`LOOM_TOKEN`).
    pub token: String,
    /// Codex·OpenCode 2 노드의 서버.
    pub server: Option<ServerLink>,
    /// 노드를 띄운 Claude·Codex 계정. 시스템 기본과 다른 프로필은 없다. 계정 전환이 옮길 노드를 고를 때 쓴다.
    pub account: Option<AccountId>,
}

/// 살아 있는 노드 프로세스. 완료한 노드도 포함한다. 데몬이 시작할 때 다시 붙고, 계정 전환이 다시 띄운다.
pub struct Active {
    pub id: NodeId,
    pub name: String,
    pub agent: Option<AgentId>,
    pub server: Option<ServerLink>,
    pub account: Option<AccountId>,
    /// Claude·Cursor 세션의 기록 파일. 파일 이름이 세션 id다. `Graph::set_transcript`가 남긴다.
    pub transcript: Option<String>,
    pub model: Model,
    /// 이 프로세스가 데몬을 부를 때 내는 토큰(`Launched::token`).
    pub token: String,
}

impl Graph {
    /// 사람이 띄운 노드(깊이 0)를 더한다. 이름은 `<에이전트 id>-<번호>`로 짓는다(`root_name`).
    /// `start`는 노드 프로세스를 띄운다. 실패하면 노드를 남기지 않는다(`started`).
    pub fn add_root(
        &self,
        place: &Place,
        agent: Option<AgentId>,
        launched: &Launched,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        let id = {
            let mut db = self.lock();
            let tx = db.transaction()?;
            let name = root_name(&tx, agent.as_ref().map_or(SHELL_NAME, |a| &a.0))?;
            let id = insert(&tx, &name, &agent, None, place)?;
            keep(&tx, id, launched)?;
            tx.commit()?;
            id
        };
        self.started(id, start)?;
        Ok(id)
    }

    /// `parent`의 자식을 더하고 `brief`를 자식 메일함의 첫 메시지로 넣는다. `start`가 실패하면 노드를 남기지 않는다(`started`).
    pub fn add_child(
        &self,
        parent: NodeId,
        child: NewChild,
        limits: TreeLimits,
        launched: &Launched,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        let NewChild {
            name,
            worktree,
            agent,
            brief,
            role,
        } = child;
        let name = checked_name(name)?;
        if brief.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(brief.len()));
        }
        let mut db = self.lock();
        let tx = db.transaction()?;
        if let Some(max) = limits.max_depth
            && ancestors(&tx, parent)?.len() as u32 >= max
        {
            return Err(Refusal::TooDeep);
        }
        check_children(&tx, parent, limits)?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM nodes WHERE name = ?2
             AND (parent = ?1 OR id = ?1 OR id = (SELECT parent FROM nodes WHERE id = ?1)))",
            params![parent.0, name],
            |row| row.get(0),
        )?;
        if taken {
            return Err(Refusal::NameTaken(name.to_owned()));
        }
        let place = place_of(&tx, parent)?;
        let place = Place {
            worktree: worktree.map_or(place.worktree, str::to_owned),
            ..place
        };
        let id = insert(&tx, name, &agent, Some(parent), &place)?;
        tx.execute(
            "UPDATE nodes SET role = ?2 WHERE id = ?1",
            params![id.0, role],
        )?;
        let has_brief = !brief.trim().is_empty();
        if has_brief {
            mailbox::enqueue(
                &tx,
                parent,
                Some(id),
                Kind::Queue,
                brief,
                Default::default(),
            )?;
        }
        keep(&tx, id, launched)?;
        tx.commit()?;
        drop(db);
        self.started(id, start)?;
        if has_brief {
            self.publish_message(id, Toward::Child);
        }
        Ok(id)
    }

    /// 노드 줄을 커밋한 뒤 잠금 밖에서 프로세스를 띄운다. 토큰은 이미 남겼으므로 노드가 뜨자마자 데몬을 불러도 찾는다.
    /// 띄우기(pty-host 시작)는 바쁜 PC에서 1초를 넘었다. 그동안 잠금을 쥐면 [확인] 같은 모든 요청이 기다렸고, 다른 연결의 쓰기는 5초 뒤 `database is locked`로 실패했다(2026-10-22 ktasearch-총괄).
    /// 화면에는 띄운 뒤에 보인다. 먼저 보이면 터미널이 PTY 없음(`Gone`)을 받고 다시 붙지 않는다.
    fn started(
        &self,
        id: NodeId,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<(), Refusal> {
        match start(id) {
            Ok(()) => match self.publish_node(&self.lock(), id) {
                // 바로 끝난 설정 스크립트는 이미 지워졌다(`discard_finished_shell`).
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(()),
                published => Ok(published?),
            },
            Err(e) => {
                self.discard(self.lock(), id)?;
                Err(Refusal::Launch(e))
            }
        }
    }

    /// 정상 종료해 지워도 되는 셸만 부른다(출력이 없거나 성공한 설정 스크립트). 관계나 메시지가 있으면 보존한다.
    pub fn discard_finished_shell(&self, id: NodeId) -> rusqlite::Result<bool> {
        let db = self.lock();
        let eligible: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1 AND agent = 'null'
             AND parent IS NULL AND state = ?2
             AND NOT EXISTS(SELECT 1 FROM nodes WHERE parent = ?1)
             AND NOT EXISTS(SELECT 1 FROM messages WHERE sender = ?1 OR recipient = ?1))",
            params![id.0, to_json(&NodeState::Running)],
            |row| row.get(0),
        )?;
        if !eligible {
            return Ok(false);
        }
        self.discard(db, id)?;
        Ok(true)
    }

    /// 관계와 메시지를 살피지 않고 노드 줄을 지운다. 받은 메시지는 DB가 함께 지운다.
    fn discard(&self, mut db: Db<'_>, id: NodeId) -> rusqlite::Result<()> {
        let tx = db.transaction()?;
        events::write(
            &tx,
            &Entry::Removed {
                node: events::who(&tx, id)?,
            },
            None,
        )?;
        tx.execute("DELETE FROM nodes WHERE id = ?1", [id.0])?;
        tx.commit()?;
        self.prompts.lock().unwrap().remove(&id);
        let _ = self.events.send(GraphEvent::Removed { id });
        Ok(())
    }

    /// 완료한 노드의 요약은 남기고 프로세스 종료만 따로 적는다.
    pub fn exited(&self, id: NodeId, code: u32) -> rusqlite::Result<()> {
        self.end_process(
            id,
            NodeState::Exited { code: Some(code) },
            format!("exited({code})"),
        )
    }

    /// 데몬이 다시 붙지 못한 프로세스도 부모에게 알린다.
    pub fn lost(&self, id: NodeId) -> rusqlite::Result<()> {
        self.end_process(
            id,
            NodeState::Exited { code: None },
            "exited(unknown)".to_owned(),
        )
    }

    /// 설정 스크립트가 실패해 에이전트가 뜨지 않았다. 부모는 까닭(`output`, 설정 출력의 끝)을 함께 받는다.
    /// 노드를 지우면 터미널 출력도 사라지므로 메시지가 그 증거를 남긴다. 본문은 지시문 상한 안에서 끝 쪽을 남긴다.
    pub fn setup_failed(&self, id: NodeId, code: u32, output: &str) -> rusqlite::Result<()> {
        let head = format!("setup failed (exit {code}):\n");
        let room = BRIEF_LIMIT_BYTES.saturating_sub(head.len());
        let mut start = output.len().saturating_sub(room);
        while !output.is_char_boundary(start) {
            start += 1;
        }
        self.end_process(id, NodeState::SetupFailed { code }, head + &output[start..])
    }

    fn end_process(&self, id: NodeId, ended: NodeState, notice: String) -> rusqlite::Result<()> {
        let mut db = self.lock();
        self.prompts.lock().unwrap().remove(&id);
        let Some(state) = state_of(&db, id).optional()? else {
            return Ok(());
        };
        let tx = db.transaction()?;
        let changed = tx.execute(
            "UPDATE nodes SET process_exited = 1, wait = NULL WHERE id = ?1 AND process_exited = 0",
            [id.0],
        )?;
        if changed == 0 {
            return Ok(());
        }
        waits::drop_questions(&tx, id, ClosedBy::Exit)?;
        let parent = parent_of(&tx, id)?;
        if state == NodeState::Running {
            set_state(&tx, id, &ended)?;
            mailbox::enqueue(&tx, id, parent, Kind::Queue, &notice, Default::default())?;
        }
        tx.commit()?;
        self.publish_node(&db, id)?;
        if state == NodeState::Running
            && let Some(parent) = parent
        {
            self.publish_node(&db, parent)?;
            self.publish_message(id, Toward::Parent);
        }
        Ok(())
    }

    /// 작업 완료와 무관하게 살아 있는 프로세스에 다시 붙는다.
    pub fn active(&self) -> rusqlite::Result<Vec<Active>> {
        self.lock()
            .prepare(&format!(
                "{LAUNCH_COLUMNS} WHERE process_exited = 0 ORDER BY id"
            ))?
            .query_map([], launch_row)?
            .collect()
    }

    /// 프로세스가 끝난 노드. 다시 띄우기가 남긴 세션으로 되살린다.
    pub fn ended(&self, id: NodeId) -> rusqlite::Result<Option<Active>> {
        self.lock()
            .query_row(
                &format!("{LAUNCH_COLUMNS} WHERE id = ?1 AND process_exited = 1"),
                [id.0],
                launch_row,
            )
            .optional()
    }

    /// 노드 프로세스를 다시 띄웠다(계정 전환, 다시 띄우기). 새 토큰·서버·계정을 남기고, 신호의 대기와 열린 질문을 모두 지운다. 메일함은 그대로다.
    /// 끝난 노드를 되살렸으면 다시 산 프로세스로 적고, 끝난 상태는 `running`으로 돌린다. 완료 상태는 그대로다.
    pub fn relaunched(&self, id: NodeId, launched: &Launched) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        keep(&tx, id, launched)?;
        // 옛 프로세스의 창은 사라졌다. 새 PTY에 닿기 전에 지운다. 토큰을 함께 바꾸므로 옛 프로세스의 신호는 다시 열지 못한다(`set_wait`).
        // 근거: 2026-10-23 리뷰 「다시 띄운 뒤 옛 승인」: 새 PTY를 띄운 뒤에 지우면 그 사이 옛 승인이 새 PTY에 쳤다.
        let cleared = tx.execute(
            "UPDATE nodes SET wait = NULL WHERE id = ?1 AND wait IS NOT NULL",
            [id.0],
        )? + waits::drop_questions(&tx, id, ClosedBy::Restart)?;
        let revived = tx.execute(
            "UPDATE nodes SET process_exited = 0 WHERE id = ?1 AND process_exited = 1",
            [id.0],
        )? > 0;
        if revived
            && matches!(
                state_of(&tx, id)?,
                NodeState::Exited { .. } | NodeState::SetupFailed { .. }
            )
        {
            set_state(&tx, id, &NodeState::Running)?;
        }
        tx.commit()?;
        if revived || cleared > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    /// Claude 노드 세션의 기록 파일. 훅마다 알린다. 이 기능 전에 뜬 노드도 다음 훅에서 남는다.
    pub fn set_transcript(&self, id: NodeId, path: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET transcript = ?2 WHERE id = ?1 AND transcript IS NOT ?2",
            params![id.0, path],
        )?;
        Ok(())
    }

    /// 이름을 추측하지 않고 현재 부모와 자식의 번호를 찾는다.
    pub fn relations(&self, node: NodeId) -> Result<Relations, Refusal> {
        let db = self.lock();
        let read = |id: NodeId| -> rusqlite::Result<Peer> {
            db.query_row(
                "SELECT name, state, process_exited FROM nodes WHERE id = ?1",
                [id.0],
                |row| {
                    Ok(Peer {
                        id,
                        name: row.get(0)?,
                        state: from_json(row, 1)?,
                        alive: !row.get::<_, bool>(2)?,
                    })
                },
            )
        };
        let children = db
            .prepare("SELECT id FROM nodes WHERE parent = ?1 ORDER BY id")?
            .query_map([node.0], |row| row.get::<_, u32>(0).map(NodeId))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Relations {
            me: read(node)?,
            parent: parent_of(&db, node)?.map(read).transpose()?,
            children: children
                .into_iter()
                .map(read)
                .collect::<rusqlite::Result<_>>()?,
        })
    }

    /// `loom status --all`: 그래프의 모든 노드. 부모·자식이 아닌 노드에 보낼 번호를 찾는다.
    pub fn directory(&self) -> Result<Vec<Listed>, Refusal> {
        let db = self.lock();
        let listed = db
            .prepare(
                "SELECT n.id, n.name, n.parent, coalesce(r.path, ''), coalesce(n.worktree, ''),
                 n.state, n.process_exited
                 FROM nodes n LEFT JOIN repos r ON r.id = n.repo ORDER BY n.id",
            )?
            .query_map([], |row| {
                Ok(Listed {
                    id: NodeId(row.get(0)?),
                    name: row.get(1)?,
                    parent: row.get::<_, Option<u32>>(2)?.map(NodeId),
                    place: place_label(&row.get::<_, String>(3)?, &row.get::<_, String>(4)?),
                    state: from_json(row, 5)?,
                    alive: !row.get::<_, bool>(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(listed)
    }

    /// 토큰을 낸 노드. 지운 노드의 토큰은 없다.
    pub fn caller(&self, token: &str) -> rusqlite::Result<Option<NodeId>> {
        caller_in(&self.lock(), token)
    }

    /// 노드 서버에 다시 붙는 값을 바꾼다. 예: Codex TUI가 노드 스레드를 만들었다.
    pub fn set_server(&self, id: NodeId, server: &ServerLink) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET server = ?2 WHERE id = ?1",
            params![id.0, to_json(server)],
        )?;
        Ok(())
    }

    /// 노드의 모델과 추론 강도를 바꾼다. 값이 바뀌었을 때만 화면에 알린다. 없는 노드면 아무것도 하지 않는다.
    pub fn set_model(&self, id: NodeId, model: &Model) -> rusqlite::Result<()> {
        let db = self.lock();
        let changed = db.execute(
            "UPDATE nodes SET model = ?2, effort = ?3
             WHERE id = ?1 AND (model IS NOT ?2 OR effort IS NOT ?3)",
            params![id.0, model.model, model.effort],
        )?;
        if changed > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    /// 사람이 노드를 북마크한다. 이미 북마크했으면 차례를 그대로 둔다.
    pub fn bookmark(&self, id: NodeId) -> Result<(), Refusal> {
        self.set_bookmark(
            id,
            "UPDATE nodes SET bookmark = (SELECT coalesce(max(bookmark), 0) + 1 FROM nodes)
             WHERE id = ?1 AND bookmark IS NULL",
        )
    }

    /// 사람이 노드를 북마크에서 뺀다.
    pub fn unbookmark(&self, id: NodeId) -> Result<(), Refusal> {
        self.set_bookmark(
            id,
            "UPDATE nodes SET bookmark = NULL WHERE id = ?1 AND bookmark IS NOT NULL",
        )
    }

    fn set_bookmark(&self, id: NodeId, update: &str) -> Result<(), Refusal> {
        let db = self.lock();
        name_of(&db, id)
            .optional()?
            .ok_or_else(|| Refusal::NotFound(format!("#{}", id.0)))?;
        if db.execute(update, [id.0])? > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    /// 노드 터미널 화면의 승인 창을 바꾼다. 값이 바뀌었을 때만 화면에 알린다. 기억에만 둔다.
    /// 노드마다 화면을 따라가는 작업 하나(`daemon::follow_screen`)만 부른다.
    pub fn set_prompt(&self, id: NodeId, prompt: Option<Prompt>) -> rusqlite::Result<()> {
        // 화면에 보내는 순서가 DB 변경 순서와 같도록 DB를 먼저 잠근다.
        let db = self.lock();
        let mut prompts = self.prompts.lock().unwrap();
        let changed = match prompt {
            Some(prompt) => prompts.insert(id, prompt.clone()) != Some(prompt),
            None => prompts.remove(&id).is_some(),
        };
        drop(prompts);
        if changed {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }
}

impl Graph {
    /// `id` 노드 하나를 지운다. 받은 메시지와 보관한 메시지도 지운다. 없는 노드면 아무것도 하지 않는다.
    /// 노드 프로세스는 끝내지 않는다. 부르는 쪽이 끝낸다.
    /// 자식은 부모 없이 남는다. 자식이 `id`에게 보냈지만 아직 전하지 않은 큐 메시지는 자식이 다시 보관한다.
    /// 실행 중이던 노드를 지우고 부모가 남으면 부모 메일함에 `removed` 한 줄을 넣는다.
    /// 이 줄은 메일함 상한을 넘어도 넣는다. 버리면 부모가 끝없이 기다린다.
    pub fn remove(&self, id: NodeId) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(removal) = remove_in(&tx, id)? else {
            return Ok(());
        };
        tx.commit()?;
        removal.publish(self, &db)
    }

    /// `loom rm`: `caller`가 자기 자식 `name`(이름이나 `#번호`)을 지운다. 그 자식의 자식은 같은 트랜잭션에서 `caller`가 데려간다.
    /// 하나라도 데려갈 수 없으면 아무것도 바꾸지 않고 거절한다. 돌려준 값은 지운 노드와 데려간 노드 이름이다.
    pub fn remove_child(
        &self,
        caller: NodeId,
        name: &str,
        limits: TreeLimits,
    ) -> Result<(NodeId, Vec<String>), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let id = mailbox::related(&tx, caller, name)?.0;
        if parent_of(&tx, id)? != Some(caller) {
            return Err(Refusal::NotChild(name.to_owned()));
        }
        let removal = remove_in(&tx, id)?.ok_or_else(|| Refusal::NotFound(name.to_owned()))?;
        let mut adoptions = Vec::new();
        let mut moved = Vec::new();
        let mut refused = Vec::new();
        for &child in &removal.children {
            let child_name = name_of(&tx, child)?;
            match adopt_in(&tx, caller, child, limits) {
                Ok(adoption) => {
                    adoptions.extend(adoption);
                    moved.push(child_name);
                }
                Err(Refusal::NameTaken(_)) => refused.push(format!(
                    "{child_name} (you, your parent or a child of yours has this name)"
                )),
                Err(e) => refused.push(format!("{child_name} ({e})")),
            }
        }
        if !refused.is_empty() {
            return Err(Refusal::ChildrenStay(format!(
                "{name}: {}",
                refused.join(", ")
            )));
        }
        tx.commit()?;
        removal.publish(self, &db)?;
        for adoption in adoptions {
            adoption.publish(self, &db)?;
        }
        Ok((id, moved))
    }

    /// 그래프 전체에서 이름이 `name`인 노드 하나를 찾는다.
    pub fn find(&self, name: &str) -> Result<NodeId, Refusal> {
        let db = self.lock();
        let ids = db
            .prepare("SELECT id FROM nodes WHERE name = ?1 ORDER BY id")?
            .query_map([name], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match ids[..] {
            [id] => Ok(id),
            [] => Err(Refusal::NotFound(name.to_owned())),
            _ => Err(ambiguous(&db, name, &ids)?),
        }
    }

    /// 사람이 노드 이름을 바꾼다. 이름이 그대로면 아무것도 하지 않는다.
    /// 메시지를 이름으로 먼저 찾는 범위(부모·자식, `mailbox::related`)에서 겹치지 않게 한다: 부모, 조부모, 형제(부모가 없으면 다른 뿌리), 자식, 손자.
    /// 부모와 자식이 옛 이름으로 보내면 받는 노드를 찾지 못하므로 그들 메일함에 바뀐 이름을 한 줄 넣는다.
    pub fn rename(&self, id: NodeId, name: &str) -> Result<(), Refusal> {
        let name = checked_name(name)?;
        let mut db = self.lock();
        let tx = db.transaction()?;
        let old = name_of(&tx, id)
            .optional()?
            .ok_or_else(|| Refusal::NotFound(format!("#{}", id.0)))?;
        if old == name {
            return Ok(());
        }
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM nodes n, nodes me WHERE me.id = ?1 AND n.id != ?1 AND n.name = ?2 AND (
                n.id = me.parent
                OR n.id = (SELECT parent FROM nodes WHERE id = me.parent)
                OR n.parent IS me.parent
                OR n.parent = ?1
                OR n.parent IN (SELECT id FROM nodes WHERE parent = ?1)))",
            params![id.0, name],
            |row| row.get(0),
        )?;
        if taken {
            return Err(Refusal::NameTaken(name.to_owned()));
        }
        tx.execute(
            "UPDATE nodes SET name = ?2 WHERE id = ?1",
            params![id.0, name],
        )?;
        events::write(
            &tx,
            &Entry::Renamed {
                node: events::who(&tx, id)?,
                from: old.clone(),
            },
            None,
        )?;
        let relatives = tx
            .prepare(
                "SELECT id FROM nodes WHERE parent = ?1
                 OR id = (SELECT parent FROM nodes WHERE id = ?1) ORDER BY id",
            )?
            .query_map([id.0], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let notice = format!(
            "renamed: {old} -> {name} (#{0}). Address it as {name} or '#{0}'.",
            id.0
        );
        for &to in &relatives {
            mailbox::enqueue(&tx, id, Some(to), Kind::Queue, &notice, Default::default())?;
        }
        tx.commit()?;
        self.publish_node(&db, id)?;
        let parent = parent_of(&db, id)?;
        for &to in &relatives {
            self.publish_node(&db, to)?;
            // 간선은 자식 id로 가리킨다.
            if Some(to) == parent {
                self.publish_message(id, Toward::Parent);
            } else {
                self.publish_message(to, Toward::Child);
            }
        }
        Ok(())
    }

    /// `parent`를 `child`의 새 부모로 정한다. 이미 그 부모면 아무것도 하지 않는다. 규칙은 `adopt_in`에 있다.
    pub fn adopt(&self, parent: NodeId, child: NodeId, limits: TreeLimits) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(adoption) = adopt_in(&tx, parent, child, limits)? else {
            return Ok(());
        };
        tx.commit()?;
        Ok(adoption.publish(self, &db)?)
    }
}

/// 커밋 뒤 알릴 노드 제거.
struct Removal {
    id: NodeId,
    children: Vec<NodeId>,
    waiting_parent: Option<NodeId>,
}

impl Removal {
    fn publish(self, graph: &Graph, db: &Connection) -> rusqlite::Result<()> {
        graph.prompts.lock().unwrap().remove(&self.id);
        let _ = graph.events.send(GraphEvent::Removed { id: self.id });
        for child in self.children {
            graph.publish_node(db, child)?;
        }
        if let Some(parent) = self.waiting_parent {
            graph.publish_node(db, parent)?;
            graph.mail.notify_waiters();
        }
        Ok(())
    }
}

/// 노드 하나를 지운다. 자식은 뿌리가 된다. 노드가 없으면 `None`이다.
fn remove_in(tx: &Connection, id: NodeId) -> rusqlite::Result<Option<Removal>> {
    let Some(state) = state_of(tx, id).optional()? else {
        return Ok(None);
    };
    let children = tx
        .prepare("SELECT id FROM nodes WHERE parent = ?1 ORDER BY id")?
        .query_map([id.0], |row| Ok(NodeId(row.get(0)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let waiting_parent = parent_of(tx, id)?.filter(|_| state == NodeState::Running);
    // 보낸 노드 이름을 읽으므로 지우기 전에 넣는다.
    if let Some(parent) = waiting_parent {
        mailbox::enqueue(
            tx,
            id,
            Some(parent),
            Kind::Queue,
            "removed",
            Default::default(),
        )?;
    }
    tx.execute(
        "UPDATE messages SET recipient = NULL
         WHERE recipient = ?1 AND delivered = 0 AND kind = ?2
         AND sender IN (SELECT id FROM nodes WHERE parent = ?1)",
        params![id.0, to_json(&Kind::Queue)],
    )?;
    tx.execute(
        "DELETE FROM messages WHERE recipient IS NULL AND sender = ?1",
        [id.0],
    )?;
    events::write(
        tx,
        &Entry::Removed {
            node: events::who(tx, id)?,
        },
        None,
    )?;
    tx.execute("UPDATE nodes SET parent = NULL WHERE parent = ?1", [id.0])?;
    tx.execute("DELETE FROM nodes WHERE id = ?1", [id.0])?;
    Ok(Some(Removal {
        id,
        children,
        waiting_parent,
    }))
}

/// 커밋 뒤 알릴 입양.
struct Adoption {
    parent: NodeId,
    child: NodeId,
    told_old: Option<NodeId>,
}

impl Adoption {
    fn publish(self, graph: &Graph, db: &Connection) -> rusqlite::Result<()> {
        for node in [Some(self.child), Some(self.parent), self.told_old]
            .into_iter()
            .flatten()
        {
            graph.publish_node(db, node)?;
        }
        graph.publish_message(self.child, Toward::Child);
        Ok(())
    }
}

/// `parent`를 `child`의 새 부모로 정한다. 이미 그 부모면 `None`이다. 거절하면 아무것도 쓰지 않는다.
/// `child`가 보관한 메시지는 보낸 순서대로 새 부모 메일함에 넣는다. 보관할 때 상한을 봤으므로 메일함 상한은 보지 않는다.
/// 실행 중인 노드를 데려가면 옛 부모 메일함에 `adopted: <새 부모>` 한 줄을 넣는다. 옛 부모가 끝없이 기다리지 않게 한다.
fn adopt_in(
    tx: &Connection,
    parent: NodeId,
    child: NodeId,
    limits: TreeLimits,
) -> Result<Option<Adoption>, Refusal> {
    let old = parent_of(tx, child)?;
    if old == Some(parent) {
        return Ok(None);
    }
    let above = ancestors(tx, parent)?;
    if parent == child || above.contains(&child) {
        return Err(Refusal::Cycle);
    }
    if let Some(max) = limits.max_depth
        && above.len() as u32 + 1 + height(tx, child)? > max
    {
        return Err(Refusal::TooDeep);
    }
    check_children(tx, parent, limits)?;
    let name = name_of(tx, child)?;
    let taken: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM nodes WHERE id != ?3 AND
            ((name = ?2 AND (parent = ?1 OR id = ?1 OR id = (SELECT parent FROM nodes WHERE id = ?1)))
             OR (parent = ?3 AND name = (SELECT name FROM nodes WHERE id = ?1))))",
        params![parent.0, name, child.0],
        |row| row.get(0),
    )?;
    if taken {
        return Err(Refusal::NameTaken(name));
    }
    let told_old = old.filter(|_| state_of(tx, child).ok() == Some(NodeState::Running));
    if let Some(old) = told_old {
        let notice = format!("adopted: {}", name_of(tx, parent)?);
        mailbox::enqueue(
            tx,
            child,
            Some(old),
            Kind::Queue,
            &notice,
            Default::default(),
        )?;
    }
    tx.execute(
        "UPDATE nodes SET parent = ?2 WHERE id = ?1",
        params![child.0, parent.0],
    )?;
    mailbox::enqueue(
        tx,
        parent,
        Some(child),
        Kind::Queue,
        &format!(
            "parent: {} (#{}). Use loom status to inspect current relationships.",
            name_of(tx, parent)?,
            parent.0
        ),
        Default::default(),
    )?;
    let moved = tx.execute(
        "UPDATE messages SET recipient = ?2 WHERE recipient IS NULL AND sender = ?1",
        params![child.0, parent.0],
    )?;
    events::write(
        tx,
        &Entry::Adopted {
            node: events::who(tx, child)?,
            from: old.map(|old| events::who(tx, old)).transpose()?,
            to: events::who(tx, parent)?,
            held: moved as u32,
        },
        None,
    )?;
    Ok(Some(Adoption {
        parent,
        child,
        told_old,
    }))
}

/// `<prefix>-<번호>`. 번호는 사람이 띄운 노드 사이에서 겹치지 않는 가장 작은 수다.
fn root_name(db: &Connection, prefix: &str) -> rusqlite::Result<String> {
    let mut number = 1;
    loop {
        let name = format!("{prefix}-{number}");
        let taken: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM nodes WHERE parent IS NULL AND name = ?1)",
            [&name],
            |row| row.get(0),
        )?;
        if !taken {
            return Ok(name);
        }
        number += 1;
    }
}

fn optional_json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row,
    idx: usize,
) -> rusqlite::Result<Option<T>> {
    match row.get::<_, Option<String>>(idx)? {
        Some(_) => from_json(row, idx).map(Some),
        None => Ok(None),
    }
}

fn checked_name(name: &str) -> Result<&str, Refusal> {
    match name.trim() {
        "" => Err(Refusal::EmptyName),
        name if name.starts_with('#')
            || name == mailbox::PARENT
            || name.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Err(Refusal::ReservedName(name.to_owned()))
        }
        name => Ok(name),
    }
}

const LAUNCH_COLUMNS: &str =
    "SELECT id, name, agent, server, account, transcript, model, effort, token FROM nodes";

fn launch_row(row: &Row) -> rusqlite::Result<Active> {
    Ok(Active {
        id: NodeId(row.get(0)?),
        name: row.get(1)?,
        agent: from_json(row, 2)?,
        server: optional_json(row, 3)?,
        account: optional_json(row, 4)?,
        transcript: row.get(5)?,
        model: Model {
            model: row.get(6)?,
            effort: row.get(7)?,
        },
        token: row.get(8)?,
    })
}

pub(super) fn caller_in(db: &Connection, token: &str) -> rusqlite::Result<Option<NodeId>> {
    db.query_row("SELECT id FROM nodes WHERE token = ?1", [token], |row| {
        Ok(NodeId(row.get(0)?))
    })
    .optional()
}

fn keep(db: &Connection, id: NodeId, launched: &Launched) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET token = ?2, server = ?3, account = ?4 WHERE id = ?1",
        params![
            id.0,
            launched.token,
            launched.server.as_ref().map(to_json),
            launched.account.as_ref().map(to_json)
        ],
    )?;
    Ok(())
}

fn insert(
    db: &Connection,
    name: &str,
    agent: &Option<AgentId>,
    parent: Option<NodeId>,
    place: &Place,
) -> Result<NodeId, Refusal> {
    // 저장소 줄이 있을 때만 넣는다. 저장소 빼기(`Repos::remove`)도 검사와 삭제가 한 문장이라 둘 사이에 노드가 남지 않는다.
    let inserted = db.execute(
        "INSERT INTO nodes (name, agent, parent, state, repo, worktree)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM repos WHERE id = ?5)",
        params![
            name,
            to_json(agent),
            parent.map(|p| p.0),
            to_json(&NodeState::Running),
            place.repo.0,
            place.worktree,
        ],
    )?;
    if inserted == 0 {
        return Err(Refusal::NoRepo);
    }
    let id = NodeId(db.last_insert_rowid() as u32);
    let entry = Entry::Created {
        node: events::Who {
            id: Some(id),
            name: name.to_owned(),
        },
        parent: parent.map(|p| events::who(db, p)).transpose()?,
        agent: agent.clone(),
    };
    events::write(db, &entry, None)?;
    Ok(id)
}

/// `parent`가 자식을 하나 더 받을 수 있는지 본다. 끝난 자식도 센다. 사람이 지워야 자리가 난다.
fn check_children(db: &Connection, parent: NodeId, limits: TreeLimits) -> Result<(), Refusal> {
    let Some(max) = limits.max_children else {
        return Ok(());
    };
    let count: u32 = db.query_row(
        "SELECT count(*) FROM nodes WHERE parent = ?1",
        [parent.0],
        |row| row.get(0),
    )?;
    if count >= max {
        return Err(Refusal::TooManyChildren(max));
    }
    Ok(())
}

/// 부모부터 뿌리까지. 개수가 깊이다.
fn ancestors(db: &Connection, mut id: NodeId) -> rusqlite::Result<Vec<NodeId>> {
    let mut found = Vec::new();
    while let Some(parent) = parent_of(db, id)? {
        found.push(parent);
        id = parent;
    }
    Ok(found)
}

/// `id` 아래로 가장 깊은 자손까지의 단계 수. 자식이 없으면 0이다.
fn height(db: &Connection, id: NodeId) -> rusqlite::Result<u32> {
    db.query_row(
        "WITH RECURSIVE below(id, level) AS (
            SELECT id, 0 FROM nodes WHERE id = ?1
            UNION ALL SELECT n.id, b.level + 1 FROM nodes n JOIN below b ON n.parent = b.id)
        SELECT max(level) FROM below",
        [id.0],
        |row| row.get(0),
    )
}

/// 에이전트가 조회하는 관계 정보. 프로세스 생존은 작업 완료와 별개다.
#[derive(Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: NodeId,
    pub name: String,
    pub state: NodeState,
    pub alive: bool,
}

/// `loom status --all`의 한 줄. `place`는 `<저장소 폴더>/<워크트리 폴더>`다.
#[derive(Debug, Serialize, Deserialize)]
pub struct Listed {
    pub id: NodeId,
    pub name: String,
    pub parent: Option<NodeId>,
    pub place: String,
    pub state: NodeState,
    pub alive: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Relations {
    pub me: Peer,
    pub parent: Option<Peer>,
    pub children: Vec<Peer>,
}
