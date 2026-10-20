//! 노드를 더하고 지우고, 부모를 바꾸고(입양), 프로세스가 끝난 노드를 고친다.
//! 근거: docs/design.md 「노드 제거와 입양」.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::events::{self, Entry};
use super::{
    Graph, GraphEvent, Kind, Model, NodeId, NodeState, Place, Refusal, TreeLimits, first_line,
    from_json, mailbox, name_of, parent_of, place_of, set_state, state_of, to_json,
};
use crate::profile::{AgentId, BRIEF_LIMIT_BYTES, ServerLink};

/// `Graph::add_child`가 더할 자식.
pub struct NewChild<'a> {
    pub name: &'a str,
    /// 없으면 부모와 같은 워크트리에 든다. 저장소는 늘 부모와 같다.
    pub worktree: Option<&'a str>,
    pub agent: Option<AgentId>,
    pub brief: &'a str,
}

/// 에이전트 없이 띄운 노드(셸) 이름의 앞부분.
const SHELL_NAME: &str = "shell";

/// 띄운 노드와 함께 남기는 값. 데몬이 다시 시작해도 노드에 다시 붙게 한다.
pub struct Launched {
    /// 노드가 데몬을 부를 때 내는 토큰(`LOOM_TOKEN`).
    pub token: String,
    /// Codex·OpenCode 2 노드의 서버.
    pub server: Option<ServerLink>,
}

/// 데몬이 시작할 때 다시 붙을 프로세스. 완료한 노드도 포함한다.
pub struct Active {
    pub id: NodeId,
    pub agent: Option<AgentId>,
    pub server: Option<ServerLink>,
}

impl Graph {
    /// 사람이 띄운 노드(깊이 0)를 더한다. 이름은 `<에이전트 id>-<번호>`로 짓는다(`root_name`).
    /// `brief`는 사람이 쓴 첫 지시다. CLI에 인자로 넘기므로 메일함에는 넣지 않고 카드 주제에만 쓴다.
    /// `launch`가 실패하면 노드를 남기지 않는다.
    pub fn add_root(
        &self,
        place: &Place,
        agent: Option<AgentId>,
        brief: &str,
        launch: impl FnOnce(NodeId) -> Result<Launched, String>,
    ) -> Result<NodeId, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let name = root_name(&tx, agent.as_ref().map_or(SHELL_NAME, |a| &a.0))?;
        let id = insert(&tx, &name, &agent, None, place, brief)?;
        keep(&tx, id, &launch(id).map_err(Refusal::Launch)?)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        Ok(id)
    }

    /// `parent`의 자식을 더하고 `brief`를 자식 메일함의 첫 메시지로 넣는다. `launch`가 실패하면 아무것도 남기지 않는다.
    pub fn add_child(
        &self,
        parent: NodeId,
        child: NewChild,
        limits: TreeLimits,
        launch: impl FnOnce(NodeId) -> Result<Launched, String>,
    ) -> Result<NodeId, Refusal> {
        let NewChild {
            name,
            worktree,
            agent,
            brief,
        } = child;
        let name = checked_name(name)?;
        if brief.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(brief.len()));
        }
        let mut db = self.lock();
        let tx = db.transaction()?;
        if ancestors(&tx, parent)?.len() as u32 >= limits.max_depth {
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
        let id = insert(&tx, name, &agent, Some(parent), &place, brief)?;
        let has_brief = !brief.trim().is_empty();
        if has_brief {
            mailbox::enqueue(&tx, parent, Some(id), Kind::Queue, brief)?;
        }
        keep(&tx, id, &launch(id).map_err(Refusal::Launch)?)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        if has_brief {
            self.publish_message(id);
        }
        Ok(id)
    }

    /// 출력 없이 정상 종료한 셸만 부른다. 관계나 메시지가 있으면 보존한다.
    pub fn discard_empty_shell(&self, id: NodeId) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let eligible: bool = tx.query_row(
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
        events::write(
            &tx,
            &Entry::Removed {
                node: events::who(&tx, id)?,
            },
            None,
        )?;
        tx.execute("DELETE FROM nodes WHERE id = ?1", [id.0])?;
        tx.commit()?;
        self.titles.lock().unwrap().remove(&id);
        let _ = self.events.send(GraphEvent::Removed { id });
        Ok(true)
    }

    /// 완료한 노드의 요약은 남기고 프로세스 종료만 따로 적는다.
    pub fn exited(&self, id: NodeId, code: u32) -> rusqlite::Result<()> {
        self.end_process(id, Some(code))
    }

    /// 데몬이 다시 붙지 못한 프로세스도 부모에게 알린다.
    pub fn lost(&self, id: NodeId) -> rusqlite::Result<()> {
        self.end_process(id, None)
    }

    fn end_process(&self, id: NodeId, code: Option<u32>) -> rusqlite::Result<()> {
        let mut db = self.lock();
        self.titles.lock().unwrap().remove(&id);
        let Some(state) = state_of(&db, id).optional()? else {
            return Ok(());
        };
        let tx = db.transaction()?;
        let changed = tx.execute(
            "UPDATE nodes SET process_exited = 1 WHERE id = ?1 AND process_exited = 0",
            [id.0],
        )?;
        if changed == 0 {
            return Ok(());
        }
        let parent = parent_of(&tx, id)?;
        if state == NodeState::Running {
            set_state(&tx, id, &NodeState::Exited { code })?;
            let notice = code.map_or_else(
                || "exited(unknown)".to_owned(),
                |code| format!("exited({code})"),
            );
            mailbox::enqueue(&tx, id, parent, Kind::Queue, &notice)?;
        }
        tx.commit()?;
        self.publish_node(&db, id)?;
        if state == NodeState::Running
            && let Some(parent) = parent
        {
            self.publish_node(&db, parent)?;
            self.publish_message(id);
        }
        Ok(())
    }

    /// 작업 완료와 무관하게 살아 있는 프로세스에 다시 붙는다.
    pub fn active(&self) -> rusqlite::Result<Vec<Active>> {
        self.lock()
            .prepare("SELECT id, agent, server FROM nodes WHERE process_exited = 0 ORDER BY id")?
            .query_map([], |row| {
                let server: Option<String> = row.get(2)?;
                Ok(Active {
                    id: NodeId(row.get(0)?),
                    agent: from_json(row, 1)?,
                    server: match server {
                        Some(_) => Some(from_json(row, 2)?),
                        None => None,
                    },
                })
            })?
            .collect()
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

    /// 토큰을 낸 노드. 지운 노드의 토큰은 없다.
    pub fn caller(&self, token: &str) -> rusqlite::Result<Option<NodeId>> {
        self.lock()
            .query_row("SELECT id FROM nodes WHERE token = ?1", [token], |row| {
                Ok(NodeId(row.get(0)?))
            })
            .optional()
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

    /// 노드 CLI의 터미널 제목에서 꺼낸 주제를 바꾼다. 값이 바뀌었을 때만 화면에 알린다. 기억에만 둔다.
    /// 노드마다 PTY 출력을 읽는 스레드 하나만 부른다. 그래서 비교와 바꾸기 사이에 다른 값이 끼지 않는다.
    pub fn set_title(&self, id: NodeId, title: Option<String>) -> rusqlite::Result<()> {
        if self.titles.lock().unwrap().get(&id) == title.as_ref() {
            return Ok(());
        }
        // 화면에 보내는 순서가 DB 변경 순서와 같도록 DB를 먼저 잠근다.
        let db = self.lock();
        let mut titles = self.titles.lock().unwrap();
        match title {
            Some(title) => titles.insert(id, title),
            None => titles.remove(&id),
        };
        drop(titles);
        self.publish_node(&db, id)
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
        let Some(state) = state_of(&tx, id).optional()? else {
            return Ok(());
        };
        let children = tx
            .prepare("SELECT id FROM nodes WHERE parent = ?1")?
            .query_map([id.0], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let waiting_parent = parent_of(&tx, id)?.filter(|_| state == NodeState::Running);
        // 보낸 노드 이름을 읽으므로 지우기 전에 넣는다.
        if let Some(parent) = waiting_parent {
            mailbox::enqueue(&tx, id, Some(parent), Kind::Queue, "removed")?;
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
            &tx,
            &Entry::Removed {
                node: events::who(&tx, id)?,
            },
            None,
        )?;
        tx.execute("UPDATE nodes SET parent = NULL WHERE parent = ?1", [id.0])?;
        tx.execute("DELETE FROM nodes WHERE id = ?1", [id.0])?;
        tx.commit()?;
        self.titles.lock().unwrap().remove(&id);
        let _ = self.events.send(GraphEvent::Removed { id });
        for child in children {
            self.publish_node(&db, child)?;
        }
        if let Some(parent) = waiting_parent {
            self.publish_node(&db, parent)?;
            self.mail.notify_waiters();
        }
        Ok(())
    }

    /// 그래프 전체에서 이름이 `name`인 노드 하나를 찾는다.
    pub fn find(&self, name: &str) -> Result<NodeId, Refusal> {
        let ids = self
            .lock()
            .prepare("SELECT id FROM nodes WHERE name = ?1 LIMIT 2")?
            .query_map([name], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match ids[..] {
            [id] => Ok(id),
            [] => Err(Refusal::NotFound(name.to_owned())),
            _ => Err(Refusal::Ambiguous(name.to_owned())),
        }
    }

    /// `parent`를 `child`의 새 부모로 정한다. 이미 그 부모면 아무것도 하지 않는다.
    /// `child`가 보관한 메시지는 보낸 순서대로 새 부모 메일함에 넣는다. 보관할 때 상한을 봤으므로 메일함 상한은 보지 않는다.
    /// 실행 중인 노드를 데려가면 옛 부모 메일함에 `adopted: <새 부모>` 한 줄을 넣는다. 옛 부모가 끝없이 기다리지 않게 한다.
    pub fn adopt(&self, parent: NodeId, child: NodeId, limits: TreeLimits) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let old = parent_of(&tx, child)?;
        if old == Some(parent) {
            return Ok(());
        }
        let above = ancestors(&tx, parent)?;
        if parent == child || above.contains(&child) {
            return Err(Refusal::Cycle);
        }
        if above.len() as u32 + 1 + height(&tx, child)? > limits.max_depth {
            return Err(Refusal::TooDeep);
        }
        check_children(&tx, parent, limits)?;
        let name = name_of(&tx, child)?;
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
        let told_old = old.filter(|_| state_of(&tx, child).ok() == Some(NodeState::Running));
        if let Some(old) = told_old {
            let notice = format!("adopted: {}", name_of(&tx, parent)?);
            mailbox::enqueue(&tx, child, Some(old), Kind::Queue, &notice)?;
        }
        tx.execute(
            "UPDATE nodes SET parent = ?2 WHERE id = ?1",
            params![child.0, parent.0],
        )?;
        mailbox::enqueue(
            &tx,
            parent,
            Some(child),
            Kind::Queue,
            &format!(
                "parent: {} (#{}). Use loom status to inspect current relationships.",
                name_of(&tx, parent)?,
                parent.0
            ),
        )?;
        let moved = tx.execute(
            "UPDATE messages SET recipient = ?2 WHERE recipient IS NULL AND sender = ?1",
            params![child.0, parent.0],
        )?;
        events::write(
            &tx,
            &Entry::Adopted {
                node: events::who(&tx, child)?,
                from: old.map(|old| events::who(&tx, old)).transpose()?,
                to: events::who(&tx, parent)?,
                held: moved as u32,
            },
            None,
        )?;
        tx.commit()?;
        for node in [Some(child), Some(parent), told_old].into_iter().flatten() {
            self.publish_node(&db, node)?;
        }
        self.publish_message(child);
        Ok(())
    }
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

fn checked_name(name: &str) -> Result<&str, Refusal> {
    match name.trim() {
        "" => Err(Refusal::EmptyName),
        name if name.starts_with('#') => Err(Refusal::NameTaken(name.to_owned())),
        name => Ok(name),
    }
}

fn keep(db: &Connection, id: NodeId, launched: &Launched) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET token = ?2, server = ?3 WHERE id = ?1",
        params![id.0, launched.token, launched.server.as_ref().map(to_json)],
    )?;
    Ok(())
}

fn insert(
    db: &Connection,
    name: &str,
    agent: &Option<AgentId>,
    parent: Option<NodeId>,
    place: &Place,
    brief: &str,
) -> rusqlite::Result<NodeId> {
    db.execute(
        "INSERT INTO nodes (name, agent, parent, state, repo, worktree, brief) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            name,
            to_json(agent),
            parent.map(|p| p.0),
            to_json(&NodeState::Running),
            place.repo.0,
            place.worktree,
            first_line(brief),
        ],
    )?;
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Relations {
    pub me: Peer,
    pub parent: Option<Peer>,
    pub children: Vec<Peer>,
}
