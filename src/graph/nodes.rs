use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::agent_state::{self, AgentEvent, AgentState};
use super::events::{self, Entry};
use super::{
    CardView, Db, Graph, GraphEvent, NodeId, NodeState, Place, ProcessOutcome, REACHABLE, Toward,
    Work, from_json, mailbox, name_of, node_state, note_state, optional_json, parent_of,
    place_label, place_of, process_of, state_of, to_json, waits,
};
use crate::accounts::AccountId;
use crate::protocol::{
    AgentId, BRIEF_LIMIT_BYTES, Kind, MessageKind, Model, Prompt, Refusal, ServerLink,
    StartDigests, TreeLimits,
};

pub struct NewChild<'a> {
    pub name: &'a str,
    pub worktree: Option<&'a str>,
    pub agent: Option<AgentId>,
    pub brief: &'a str,
    pub role: Option<&'a str>,
    pub task: &'a str,
    pub review: Option<&'a str>,
}

const SHELL_NAME: &str = "shell";
const NO_PARENT: &str = "(none)";

pub struct Launched {
    pub token: String,
    pub server: Option<ServerLink>,
    pub account: Option<AccountId>,
    pub rules: Option<StartDigests>,
}

pub struct Active {
    pub id: NodeId,
    pub name: String,
    pub agent: Option<AgentId>,
    pub server: Option<ServerLink>,
    pub account: Option<AccountId>,
    pub transcript: Option<String>,
    pub model: Model,
    pub token: String,
    pub role: Option<String>,
    pub rules: Option<StartDigests>,
}

impl Graph {
    pub fn add_root(
        &self,
        place: &Place,
        agent: Option<AgentId>,
        task: Option<&str>,
        launched: &Launched,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        self.insert_root(place, agent, task, false, launched, start)
    }

    pub fn add_setup(
        &self,
        place: &Place,
        launched: &Launched,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        self.insert_root(place, None, None, true, launched, start)
    }

    fn insert_root(
        &self,
        place: &Place,
        agent: Option<AgentId>,
        task: Option<&str>,
        setup: bool,
        launched: &Launched,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<NodeId, Refusal> {
        let id = {
            let mut db = self.lock();
            let tx = db.transaction()?;
            let name = root_name(&tx, agent.as_ref().map_or(SHELL_NAME, |a| &a.0))?;
            let id = insert(&tx, &name, &agent, None, place)?;
            tx.execute(
                "UPDATE nodes SET task = ?2, setup = ?3 WHERE id = ?1",
                params![id.0, task, setup],
            )?;
            keep(&tx, id, launched)?;
            tx.commit()?;
            id
        };
        self.started(id, start)?;
        Ok(id)
    }

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
            task,
            review,
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
        let place = place_of(&tx, parent)?;
        match (review, role) {
            (Some(branch), role) => {
                super::gate::check_review(&tx, parent, place.repo, branch, role)?;
            }
            (None, Some(role)) if worktree.is_none() && parent_of(&tx, parent)?.is_none() => {
                return Err(Refusal::LeadChildNeedsWorktree(role.to_owned()));
            }
            (None, _) => {}
        }
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM nodes WHERE name = ?2
             AND (parent = ?1 OR id = ?1 OR id = (SELECT parent FROM nodes WHERE id = ?1)))",
            params![parent.0, name],
            |row| row.get(0),
        )?;
        if taken {
            return Err(Refusal::NameTaken(name.to_owned()));
        }
        let place = Place {
            worktree: worktree.map_or(place.worktree, str::to_owned),
            ..place
        };
        let id = insert(&tx, name, &agent, Some(parent), &place)?;
        tx.execute(
            "UPDATE nodes SET role = ?2, task = ?3, review = ?4 WHERE id = ?1",
            params![id.0, role, task, review],
        )?;
        let has_brief = !brief.trim().is_empty();
        if has_brief {
            mailbox::enqueue(
                &tx,
                parent,
                Some(id),
                Kind::Queue,
                &MessageKind::Note,
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

    pub fn task_of(&self, id: NodeId) -> rusqlite::Result<Option<String>> {
        self.lock()
            .query_row("SELECT task FROM nodes WHERE id = ?1", [id.0], |row| {
                row.get(0)
            })
    }

    pub fn keep_task(&self, id: NodeId, task: &str) -> rusqlite::Result<String> {
        let db = self.lock();
        db.execute(
            "UPDATE nodes SET task = ?2 WHERE id = ?1 AND task IS NULL",
            params![id.0, task],
        )?;
        db.query_row("SELECT task FROM nodes WHERE id = ?1", [id.0], |row| {
            row.get(0)
        })
    }

    pub fn next_root_name(&self, agent: Option<&AgentId>) -> rusqlite::Result<String> {
        root_name(&self.lock(), agent.map_or(SHELL_NAME, |a| &a.0))
    }

    fn started(
        &self,
        id: NodeId,
        start: impl FnOnce(NodeId) -> Result<(), String>,
    ) -> Result<(), Refusal> {
        match start(id) {
            Ok(()) => match self.publish_node(&self.lock(), id) {
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(()),
                published => Ok(published?),
            },
            Err(e) => {
                self.discard(self.lock(), id)?;
                Err(Refusal::Launch(e))
            }
        }
    }

    pub fn is_setup(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1 AND setup)",
            [id.0],
            |row| row.get(0),
        )
    }

    pub fn discard_finished_shell(&self, id: NodeId) -> rusqlite::Result<bool> {
        let db = self.lock();
        let eligible: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1 AND agent = 'null'
             AND parent IS NULL AND state = ?2
             AND NOT EXISTS(SELECT 1 FROM nodes WHERE parent = ?1)
             AND NOT EXISTS(SELECT 1 FROM messages WHERE sender = ?1 OR recipient = ?1))",
            params![id.0, to_json(&Work::Running)],
            |row| row.get(0),
        )?;
        if !eligible {
            return Ok(false);
        }
        self.discard(db, id)?;
        Ok(true)
    }

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

    pub fn exited(&self, id: NodeId, code: u32) -> rusqlite::Result<()> {
        let what = |node| MessageKind::Exited {
            node,
            code: Some(code),
        };
        self.end_process(id, ProcessOutcome::Exited { code }, what, String::new())
    }

    pub fn lost(&self, id: NodeId) -> rusqlite::Result<()> {
        let what = |node| MessageKind::Exited { node, code: None };
        self.end_process(id, ProcessOutcome::Lost, what, String::new())
    }

    pub fn setup_failed(&self, id: NodeId, code: u32, output: &str) -> rusqlite::Result<()> {
        let mut start = output.len().saturating_sub(BRIEF_LIMIT_BYTES);
        while !output.is_char_boundary(start) {
            start += 1;
        }
        let what = |node| MessageKind::SetupFailed { node, code };
        let output = output[start..].to_owned();
        self.end_process(id, ProcessOutcome::SetupFailed { code }, what, output)
    }

    fn end_process(
        &self,
        id: NodeId,
        ended: ProcessOutcome,
        what: impl FnOnce(String) -> MessageKind,
        notice: String,
    ) -> rusqlite::Result<()> {
        let mut db = self.lock();
        self.prompts.lock().unwrap().remove(&id);
        let Some(state) = state_of(&db, id).optional()? else {
            return Ok(());
        };
        let tx = db.transaction()?;
        agent_state::apply(&tx, id, AgentEvent::Exited)?;
        let changed = tx.execute(
            "UPDATE nodes SET process = ?2, sleeping = 0, wait = NULL WHERE id = ?1 AND process IS NULL",
            params![id.0, to_json(&ended)],
        )?;
        if changed == 0 {
            return Ok(());
        }
        waits::drop_questions(&tx, id)?;
        let parent = parent_of(&tx, id)?;
        note_state(&tx, id, state.clone())?;
        if state == NodeState::Running {
            let what = what(name_of(&tx, id)?);
            mailbox::enqueue(
                &tx,
                id,
                parent,
                Kind::Queue,
                &what,
                &notice,
                Default::default(),
            )?;
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

    pub fn active(&self) -> rusqlite::Result<Vec<Active>> {
        self.lock()
            .prepare(&format!(
                "{LAUNCH_COLUMNS} WHERE process IS NULL ORDER BY id"
            ))?
            .query_map([], launch_row)?
            .collect()
    }

    pub fn done_children(&self) -> rusqlite::Result<Vec<NodeId>> {
        self.lock()
            .prepare(
                "SELECT id FROM nodes WHERE process IS NULL AND parent IS NOT NULL
                 AND wait IS NULL AND state ->> 'kind' = 'done' ORDER BY id",
            )?
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect()
    }

    pub fn slept(&self, id: NodeId) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        agent_state::apply(&tx, id, AgentEvent::Slept)?;
        let changed = tx.execute(
            "UPDATE nodes SET process = ?2, sleeping = 0, wait = NULL
             WHERE id = ?1 AND process IS NULL AND sleeping = 1 AND state ->> 'kind' = 'done'",
            params![id.0, to_json(&ProcessOutcome::Asleep)],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        waits::drop_questions(&tx, id)?;
        tx.commit()?;
        self.prompts.lock().unwrap().remove(&id);
        self.publish_node(&db, id)?;
        Ok(true)
    }

    pub fn begin_sleep(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .execute(
                "UPDATE nodes SET sleeping = 1 WHERE id = ?1 AND sleeping = 0 AND process IS NULL",
                [id.0],
            )
            .map(|changed| changed > 0)
    }

    pub fn cancel_sleep(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .execute(
                "UPDATE nodes SET sleeping = 0 WHERE id = ?1 AND sleeping = 1",
                [id.0],
            )
            .map(|changed| changed > 0)
    }

    pub fn sleeping(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row(
                "SELECT sleeping = 1 FROM nodes WHERE id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()
            .map(|sleeping| sleeping.unwrap_or(false))
    }

    pub fn sleep_again(&self, id: NodeId) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let changed = tx.execute(
            "UPDATE nodes SET process = ?2
             WHERE id = ?1 AND process IS NOT NULL AND state ->> 'kind' = 'done'",
            params![id.0, to_json(&ProcessOutcome::Asleep)],
        )?;
        if changed == 0 {
            return Ok(());
        }
        agent_state::apply(&tx, id, AgentEvent::Slept)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        Ok(())
    }

    pub fn process(&self, id: NodeId) -> rusqlite::Result<Option<ProcessOutcome>> {
        process_of(&self.lock(), id)
    }

    pub fn reachable(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock().query_row(
            &format!("SELECT {REACHABLE} FROM nodes WHERE id = ?1"),
            [id.0],
            |row| row.get(0),
        )
    }

    pub fn sleep_lost(&self, id: NodeId) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(from) = state_of(&tx, id).optional()? else {
            return Ok(());
        };
        agent_state::apply(&tx, id, AgentEvent::Exited)?;
        let changed = tx.execute(
            "UPDATE nodes SET process = ?2, state = ?3
             WHERE id = ?1 AND process IS NOT NULL AND state ->> 'kind' = 'done'",
            params![
                id.0,
                to_json(&ProcessOutcome::Lost),
                to_json(&Work::Running)
            ],
        )?;
        if changed == 0 {
            return Ok(());
        }
        note_state(&tx, id, from)?;
        let parent = parent_of(&tx, id)?;
        let what = MessageKind::Exited {
            node: name_of(&tx, id)?,
            code: None,
        };
        mailbox::enqueue(&tx, id, parent, Kind::Queue, &what, "", Default::default())?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        if let Some(parent) = parent {
            self.publish_node(&db, parent)?;
            self.publish_message(id, Toward::Parent);
        }
        Ok(())
    }

    pub fn ended(&self, id: NodeId) -> rusqlite::Result<Option<Active>> {
        self.lock()
            .query_row(
                &format!("{LAUNCH_COLUMNS} WHERE id = ?1 AND process IS NOT NULL"),
                [id.0],
                launch_row,
            )
            .optional()
    }

    pub fn relaunched(&self, id: NodeId, launched: &Launched) -> rusqlite::Result<()> {
        self.restarted(id, launched, |_| Ok(0))
    }

    pub fn switched(
        &self,
        id: NodeId,
        launched: &Launched,
        agent: &AgentId,
        model: &Model,
    ) -> rusqlite::Result<()> {
        self.restarted(id, launched, |tx| {
            tx.execute(
                "UPDATE nodes SET agent = ?2, model = ?3, effort = ?4, transcript = NULL WHERE id = ?1",
                params![id.0, to_json(&Some(agent)), model.model, model.effort],
            )
        })
    }

    fn restarted(
        &self,
        id: NodeId,
        launched: &Launched,
        changed: impl FnOnce(&Connection) -> rusqlite::Result<usize>,
    ) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(from) = state_of(&tx, id).optional()? else {
            return Ok(());
        };
        agent_state::apply(&tx, id, AgentEvent::Relaunched)?;
        keep(&tx, id, launched)?;
        let cleared = tx.execute(
            "UPDATE nodes SET wait = NULL WHERE id = ?1 AND wait IS NOT NULL",
            [id.0],
        )? + changed(&tx)?;
        let revived = tx.execute(
            "UPDATE nodes SET process = NULL, sleeping = 0 WHERE id = ?1 AND process IS NOT NULL",
            [id.0],
        )? > 0;
        note_state(&tx, id, from)?;
        tx.commit()?;
        if revived || cleared > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    pub fn set_transcript(&self, id: NodeId, path: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET transcript = ?2 WHERE id = ?1 AND transcript IS NOT ?2",
            params![id.0, path],
        )?;
        Ok(())
    }

    pub fn relations(&self, node: NodeId) -> Result<Relations, Refusal> {
        let db = self.lock();
        let read = |id: NodeId| -> rusqlite::Result<Peer> {
            let agent_state = agent_state::state_of(&db, id)?;
            db.query_row(
                "SELECT name, state, process FROM nodes WHERE id = ?1",
                [id.0],
                |row| {
                    let process = optional_json(row, 2)?;
                    Ok(Peer {
                        id,
                        name: row.get(0)?,
                        state: node_state(from_json(row, 1)?, process),
                        alive: process.is_none(),
                        agent_state,
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

    pub fn directory(&self) -> Result<Vec<Listed>, Refusal> {
        let db = self.lock();
        let listed = db
            .prepare(
                "SELECT n.id, n.name, n.parent, coalesce(r.path, ''), coalesce(n.worktree, ''),
                 n.state, n.process
                 FROM nodes n LEFT JOIN repos r ON r.id = n.repo ORDER BY n.id",
            )?
            .query_map([], |row| {
                let process = optional_json(row, 6)?;
                Ok(Listed {
                    id: NodeId(row.get(0)?),
                    name: row.get(1)?,
                    parent: row.get::<_, Option<u32>>(2)?.map(NodeId),
                    place: place_label(&row.get::<_, String>(3)?, &row.get::<_, String>(4)?),
                    state: node_state(from_json(row, 5)?, process),
                    alive: process.is_none(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(listed)
    }

    pub fn caller(&self, token: &str) -> rusqlite::Result<Option<NodeId>> {
        caller_in(&self.lock(), token)
    }

    pub fn set_server(&self, id: NodeId, server: &ServerLink) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET server = ?2 WHERE id = ?1",
            params![id.0, to_json(server)],
        )?;
        Ok(())
    }

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

    pub fn set_card_view(&self, id: NodeId, view: Option<CardView>) -> Result<(), Refusal> {
        let db = self.lock();
        name_of(&db, id)
            .optional()?
            .ok_or_else(|| Refusal::NotFound(format!("#{}", id.0)))?;
        let value = view.map(|v| to_json(&v));
        if db.execute(
            "UPDATE nodes SET card_view = ?2 WHERE id = ?1 AND card_view IS NOT ?2",
            params![id.0, value],
        )? > 0
        {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    pub fn add_turn_flag(&self, id: NodeId, bit: u8) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "UPDATE nodes SET turn_flags = turn_flags | ?2 WHERE id = ?1 AND turn_flags & ?2 != ?2",
                params![id.0, bit],
            )
            .map(drop)
    }

    pub fn clear_turn_flag(&self, id: NodeId, bit: u8) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "UPDATE nodes SET turn_flags = turn_flags & ~?2 WHERE id = ?1 AND turn_flags & ?2 != 0",
                params![id.0, bit],
            )
            .map(drop)
    }

    pub fn take_turn_flags(&self, id: NodeId) -> rusqlite::Result<u8> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let flags = tx
            .query_row(
                "SELECT turn_flags FROM nodes WHERE id = ?1",
                params![id.0],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if flags != 0 {
            tx.execute(
                "UPDATE nodes SET turn_flags = 0 WHERE id = ?1",
                params![id.0],
            )?;
        }
        tx.commit()?;
        Ok(flags)
    }

    pub fn set_reply(&self, id: NodeId, text: &str) -> rusqlite::Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let db = self.lock();
        if db.execute(
            "UPDATE nodes SET reply = ?2 WHERE id = ?1 AND reply IS NOT ?2",
            params![id.0, text],
        )? > 0
        {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    pub fn bookmark(&self, id: NodeId) -> Result<(), Refusal> {
        self.set_bookmark(
            id,
            "UPDATE nodes SET bookmark = (SELECT coalesce(max(bookmark), 0) + 1 FROM nodes)
             WHERE id = ?1 AND bookmark IS NULL",
        )
    }

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

    pub fn set_prompt(&self, id: NodeId, prompt: Option<Prompt>) -> rusqlite::Result<()> {
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
    pub fn remove(&self, id: NodeId) -> rusqlite::Result<Vec<NodeId>> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(removal) = remove_in(&tx, id)? else {
            return Ok(Vec::new());
        };
        tx.commit()?;
        let children = removal.children.clone();
        removal.publish(self, &db)?;
        Ok(children)
    }

    pub fn remove_childless_child(&self, id: NodeId) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let removable: bool = tx
            .query_row(
                "SELECT parent IS NOT NULL AND NOT EXISTS (SELECT 1 FROM nodes c WHERE c.parent = n.id)
                 FROM nodes n WHERE id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !removable {
            return Ok(false);
        }
        let Some(removal) = remove_in(&tx, id)? else {
            return Ok(false);
        };
        tx.commit()?;
        removal.publish(self, &db)?;
        Ok(true)
    }

    pub fn remove_child(
        &self,
        caller: NodeId,
        name: &str,
        limits: TreeLimits,
    ) -> Result<(NodeId, Vec<String>), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let id = mailbox::any_node(&tx, caller, name)?;
        if id == caller {
            return Err(Refusal::NotFound(name.to_owned()));
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

    pub fn find(&self, name: &str) -> Result<NodeId, Refusal> {
        mailbox::named(&self.lock(), name)
    }

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
            mailbox::enqueue(
                &tx,
                id,
                Some(to),
                Kind::Queue,
                &MessageKind::Note,
                &notice,
                Default::default(),
            )?;
        }
        tx.commit()?;
        self.publish_node(&db, id)?;
        let parent = parent_of(&db, id)?;
        for &to in &relatives {
            self.publish_node(&db, to)?;
            if Some(to) == parent {
                self.publish_message(id, Toward::Parent);
            } else {
                self.publish_message(to, Toward::Child);
            }
        }
        Ok(())
    }

    pub fn adopt(
        &self,
        parent: NodeId,
        child: NodeId,
        limits: TreeLimits,
    ) -> Result<bool, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(adoption) = adopt_in(&tx, parent, child, limits)? else {
            return Ok(false);
        };
        tx.commit()?;
        let was_root = adoption.was_root;
        adoption.publish(self, &db)?;
        Ok(was_root)
    }

    pub fn detach(&self, child: NodeId) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let adoption = detach_in(&tx, child)?;
        tx.commit()?;
        Ok(adoption.publish(self, &db)?)
    }
}

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

fn remove_in(tx: &Connection, id: NodeId) -> rusqlite::Result<Option<Removal>> {
    let Some(state) = state_of(tx, id).optional()? else {
        return Ok(None);
    };
    let children = tx
        .prepare("SELECT id FROM nodes WHERE parent = ?1 ORDER BY id")?
        .query_map([id.0], |row| Ok(NodeId(row.get(0)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let waiting_parent = parent_of(tx, id)?.filter(|_| state == NodeState::Running);
    if let Some(parent) = waiting_parent {
        let what = MessageKind::Removed {
            node: name_of(tx, id)?,
        };
        mailbox::enqueue(
            tx,
            id,
            Some(parent),
            Kind::Queue,
            &what,
            "",
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

struct Adoption {
    parent: NodeId,
    child: NodeId,
    told_old: Option<NodeId>,
    was_root: bool,
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

fn detach_in(tx: &Connection, child: NodeId) -> Result<Adoption, Refusal> {
    let name = name_of(tx, child)?;
    let Some(old) = parent_of(tx, child)? else {
        return Err(Refusal::AlreadyRoot(name));
    };
    let taken: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM nodes WHERE parent IS NULL AND name = ?1)",
        [&name],
        |row| row.get(0),
    )?;
    if taken {
        return Err(Refusal::NameTaken(name));
    }
    let told_old = (state_of(tx, child).ok() == Some(NodeState::Running)).then_some(old);
    let detached = MessageKind::Adopted {
        node: name,
        parent: NO_PARENT.to_owned(),
    };
    if let Some(old) = told_old {
        mailbox::enqueue(
            tx,
            child,
            Some(old),
            Kind::Queue,
            &detached,
            "",
            Default::default(),
        )?;
    }
    tx.execute("UPDATE nodes SET parent = NULL WHERE id = ?1", [child.0])?;
    mailbox::enqueue(
        tx,
        old,
        Some(child),
        Kind::Queue,
        &detached,
        "You have no parent now. Your loom ask goes to the human inbox.",
        Default::default(),
    )?;
    events::write(
        tx,
        &Entry::Adopted {
            node: events::who(tx, child)?,
            from: Some(events::who(tx, old)?),
            to: None,
            held: 0,
        },
        None,
    )?;
    Ok(Adoption {
        parent: old,
        child,
        told_old,
        was_root: false,
    })
}

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
    let adopted = MessageKind::Adopted {
        node: name.clone(),
        parent: name_of(tx, parent)?,
    };
    if let Some(old) = told_old {
        mailbox::enqueue(
            tx,
            child,
            Some(old),
            Kind::Queue,
            &adopted,
            "",
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
        &adopted,
        &format!(
            "Your new parent is #{}. Use loom status to inspect current relationships.",
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
            to: Some(events::who(tx, parent)?),
            held: moved as u32,
        },
        None,
    )?;
    Ok(Some(Adoption {
        parent,
        child,
        told_old,
        was_root: old.is_none(),
    }))
}

pub(super) fn check_spent(
    db: &Connection,
    node: NodeId,
    keep: &[mailbox::Kept],
) -> Result<(), Refusal> {
    let spent = db
        .prepare(
            r#"SELECT c.id, c.name FROM nodes c WHERE c.parent = ?1
             AND c.state ->> 'kind' = 'done' AND c.process IS NOT NULL AND c.role IS NOT NULL
             AND NOT EXISTS (SELECT 1 FROM gates g WHERE g.state != '"merged"'
                 AND (g.owner = c.id OR (g.repo = c.repo AND g.branch = c.review)))
             ORDER BY c.id"#,
        )?
        .query_map([node.0], |row| {
            Ok((NodeId(row.get(0)?), row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(_, name)| !keep.iter().any(|k| &k.name == name))
        .collect::<Vec<_>>();
    if spent.is_empty() {
        return Ok(());
    }
    let mut inherited = Vec::new();
    for (id, _) in &spent {
        inherited.extend(
            db.prepare("SELECT name FROM nodes WHERE parent = ?1 ORDER BY id")?
                .query_map([id.0], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Err(Refusal::SpentChildren {
        children: spent.into_iter().map(|(_, name)| name).collect(),
        inherited,
    })
}

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
        name if name.starts_with('#')
            || name == mailbox::PARENT
            || name.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Err(Refusal::ReservedName(name.to_owned()))
        }
        name => Ok(name),
    }
}

const LAUNCH_COLUMNS: &str = "SELECT id, name, agent, server, account, transcript, model, effort, token, role, rules FROM nodes";

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
        role: row.get(9)?,
        rules: optional_json(row, 10)?,
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
        "UPDATE nodes SET token = ?2, server = ?3, account = ?4, rules = ?5 WHERE id = ?1",
        params![
            id.0,
            launched.token,
            launched.server.as_ref().map(to_json),
            launched.account.as_ref().map(to_json),
            launched.rules.as_ref().map(to_json)
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
    let inserted = db.execute(
        "INSERT INTO nodes (name, agent, parent, state, repo, worktree)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM repos WHERE id = ?5)",
        params![
            name,
            to_json(agent),
            parent.map(|p| p.0),
            to_json(&Work::Running),
            place.repo.0,
            place.worktree,
        ],
    )?;
    if inserted == 0 {
        return Err(Refusal::NoRepo);
    }
    let id = NodeId(db.last_insert_rowid() as u32);
    agent_state::start(db, id)?;
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

fn ancestors(db: &Connection, mut id: NodeId) -> rusqlite::Result<Vec<NodeId>> {
    let mut found = Vec::new();
    while let Some(parent) = parent_of(db, id)? {
        found.push(parent);
        id = parent;
    }
    Ok(found)
}

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

#[derive(Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: NodeId,
    pub name: String,
    pub state: NodeState,
    pub alive: bool,
    #[serde(default)]
    pub agent_state: Option<AgentState>,
}

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
