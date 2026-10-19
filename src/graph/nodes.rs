//! 노드를 더하고, 프로세스가 끝난 노드를 고친다.

use rusqlite::{Connection, params};

use super::{
    Graph, MAX_DEPTH, NodeId, NodeState, Refusal, mailbox, parent_of, set_state, state_of, to_json,
};
use crate::profile::{BRIEF_LIMIT_BYTES, Profile};

impl Graph {
    /// 사람이 띄운 노드(깊이 0)를 더한다. `launch`가 실패하면 노드를 남기지 않는다.
    pub fn add_root(
        &self,
        name: &str,
        profile: Profile,
        launch: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        let name = checked_name(name)?;
        let mut db = self.lock();
        let tx = db.transaction()?;
        let id = insert(&tx, name, profile, None)?;
        launch(id).map_err(Refusal::Launch)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        Ok(id)
    }

    /// `parent`의 자식을 더하고 `brief`를 자식 메일함의 첫 메시지로 넣는다. `launch`가 실패하면 아무것도 남기지 않는다.
    pub fn add_child(
        &self,
        parent: NodeId,
        name: &str,
        profile: Profile,
        brief: &str,
        launch: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        let name = checked_name(name)?;
        if brief.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(brief.len()));
        }
        let mut db = self.lock();
        let tx = db.transaction()?;
        if depth(&tx, parent)? >= MAX_DEPTH {
            return Err(Refusal::TooDeep);
        }
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM nodes WHERE name = ?2 AND (parent = ?1 OR id = ?1))",
            params![parent.0, name],
            |row| row.get(0),
        )?;
        if taken {
            return Err(Refusal::NameTaken(name.to_owned()));
        }
        let id = insert(&tx, name, profile, Some(parent))?;
        let has_brief = !brief.trim().is_empty();
        if has_brief {
            mailbox::enqueue(&tx, parent, id, brief)?;
        }
        launch(id).map_err(Refusal::Launch)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        if has_brief {
            self.publish_message(id);
        }
        Ok(id)
    }

    /// 프로세스가 끝났다. `done`을 보낸 노드는 그대로 둔다.
    /// 아니면 `exited`로 바꾸고 부모 메일함에 한 줄을 넣는다. 이 줄은 메일함 상한을 넘어도 넣는다. 버리면 부모가 끝없이 기다린다.
    pub fn exited(&self, id: NodeId, code: u32) -> rusqlite::Result<()> {
        let mut db = self.lock();
        if state_of(&db, id)? != NodeState::Running {
            return Ok(());
        }
        let tx = db.transaction()?;
        set_state(&tx, id, &NodeState::Exited { code: Some(code) })?;
        let parent = parent_of(&tx, id)?;
        if let Some(parent) = parent {
            mailbox::enqueue(&tx, id, parent, &format!("exited({code})"))?;
        }
        tx.commit()?;
        self.publish_node(&db, id)?;
        if let Some(parent) = parent {
            self.publish_node(&db, parent)?;
            self.publish_message(id);
        }
        Ok(())
    }
}

fn checked_name(name: &str) -> Result<&str, Refusal> {
    match name.trim() {
        "" => Err(Refusal::EmptyName),
        name => Ok(name),
    }
}

fn insert(
    db: &Connection,
    name: &str,
    profile: Profile,
    parent: Option<NodeId>,
) -> rusqlite::Result<NodeId> {
    db.execute(
        "INSERT INTO nodes (name, profile, parent, state) VALUES (?1, ?2, ?3, ?4)",
        params![
            name,
            to_json(&profile),
            parent.map(|p| p.0),
            to_json(&NodeState::Running)
        ],
    )?;
    Ok(NodeId(db.last_insert_rowid() as u32))
}

fn depth(db: &Connection, mut id: NodeId) -> rusqlite::Result<u32> {
    let mut depth = 0;
    while let Some(parent) = parent_of(db, id)? {
        depth += 1;
        id = parent;
    }
    Ok(depth)
}
