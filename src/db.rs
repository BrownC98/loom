use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use rusqlite_migration::{M, MigrationDefinitionError, Migrations};

const STEPS: &[M] = &[
    M::up(
        "CREATE TABLE nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            profile TEXT NOT NULL,
            parent INTEGER REFERENCES nodes(id),
            state TEXT NOT NULL
        );
        CREATE TABLE messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            recipient INTEGER NOT NULL REFERENCES nodes(id),
            sender INTEGER NOT NULL REFERENCES nodes(id),
            body TEXT NOT NULL,
            delivered INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX messages_queued ON messages(recipient) WHERE delivered = 0;",
    ),
    M::up(
        "ALTER TABLE nodes RENAME COLUMN profile TO agent;
        UPDATE nodes SET agent = 'null' WHERE agent = '\"command\"';",
    ),
    M::up(
        "CREATE TABLE settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            value TEXT NOT NULL
        );",
    ),
    M::up(r#"ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT '"queue"';"#),
    M::up(
        "CREATE TABLE messages_v5 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            recipient INTEGER REFERENCES nodes(id) ON DELETE CASCADE,
            sender INTEGER REFERENCES nodes(id) ON DELETE SET NULL,
            sender_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            body TEXT NOT NULL,
            delivered INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO messages_v5 (id, recipient, sender, sender_name, kind, body, delivered)
            SELECT m.id, m.recipient, m.sender, n.name, m.kind, m.body, m.delivered
            FROM messages m JOIN nodes n ON n.id = m.sender;
        DROP TABLE messages;
        ALTER TABLE messages_v5 RENAME TO messages;
        CREATE INDEX messages_queued ON messages(recipient) WHERE delivered = 0;
        CREATE INDEX messages_held ON messages(sender) WHERE recipient IS NULL;",
    ),
    M::up("ALTER TABLE messages ADD COLUMN sent_at INTEGER;"),
    M::up(
        "CREATE TABLE events (
            id INTEGER PRIMARY KEY,
            at INTEGER NOT NULL,
            entry TEXT NOT NULL,
            message INTEGER
        );",
    ),
    M::up("UPDATE settings SET value = json_remove(value, '$.defaultAgent', '$.disabledAgents');"),
    M::up("ALTER TABLE nodes ADD COLUMN model TEXT; ALTER TABLE nodes ADD COLUMN effort TEXT;"),
    M::up(
        "CREATE TABLE repos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE
        );
        ALTER TABLE nodes ADD COLUMN repo INTEGER;
        ALTER TABLE nodes ADD COLUMN worktree TEXT;",
    ),
    M::up("ALTER TABLE nodes ADD COLUMN brief TEXT;"),
    M::up(
        "UPDATE settings SET value = json_set(value, '$.cardView',
            CASE json_extract(value, '$.cardView') WHEN 'terminal' THEN 'standard' WHEN 'header' THEN 'simple' ELSE json_extract(value, '$.cardView') END)
        WHERE json_extract(value, '$.cardView') IS NOT NULL;",
    ),
    M::up("ALTER TABLE nodes ADD COLUMN token TEXT; ALTER TABLE nodes ADD COLUMN server TEXT;"),
    M::up(
        "ALTER TABLE nodes ADD COLUMN process_exited INTEGER NOT NULL DEFAULT 0;
        UPDATE nodes SET process_exited = 1 WHERE state ->> 'kind' = 'exited';
        ALTER TABLE messages ADD COLUMN reply_to INTEGER;
        CREATE INDEX messages_replies ON messages(recipient, reply_to) WHERE reply_to IS NOT NULL;",
    ),
    M::up(
        "CREATE TABLE spots (
            spot TEXT PRIMARY KEY,
            repo INTEGER NOT NULL,
            x REAL NOT NULL,
            y REAL NOT NULL
        );
        CREATE TRIGGER spots_of_repo AFTER DELETE ON repos BEGIN
            DELETE FROM spots WHERE repo = OLD.id;
        END;
        CREATE TRIGGER spots_of_node AFTER DELETE ON nodes BEGIN
            DELETE FROM spots WHERE spot ->> 'kind' = 'node' AND spot ->> 'node' = OLD.id;
        END;",
    ),
    M::up(
        "CREATE TABLE card_sizes (
            node INTEGER PRIMARY KEY,
            width REAL NOT NULL,
            height REAL NOT NULL
        );
        CREATE TRIGGER card_sizes_of_node AFTER DELETE ON nodes BEGIN
            DELETE FROM card_sizes WHERE node = OLD.id;
        END;",
    ),
    M::up(
        "CREATE TABLE arranged_repos (repo INTEGER PRIMARY KEY);
        CREATE TRIGGER arranged_of_repo AFTER DELETE ON repos BEGIN
            DELETE FROM arranged_repos WHERE repo = OLD.id;
        END;",
    ),
    M::up("ALTER TABLE nodes ADD COLUMN account TEXT; ALTER TABLE nodes ADD COLUMN transcript TEXT;"),
    M::up(
        "DELETE FROM spots WHERE spot ->> 'kind' = 'worktree';
        CREATE TRIGGER worktree_spot_of_new_node AFTER INSERT ON nodes
        WHEN NEW.parent IS NOT NULL AND (SELECT worktree FROM nodes WHERE id = NEW.parent) IS NOT NEW.worktree BEGIN
            DELETE FROM spots WHERE spot ->> 'kind' = 'worktree' AND repo = NEW.repo AND spot ->> 'path' = NEW.worktree;
        END;
        CREATE TRIGGER worktree_spot_of_parent AFTER UPDATE OF parent ON nodes
        WHEN OLD.parent IS NOT NEW.parent BEGIN
            DELETE FROM spots WHERE spot ->> 'kind' = 'worktree' AND repo = NEW.repo AND spot ->> 'path' = NEW.worktree;
        END;",
    ),
    M::up("ALTER TABLE spots ADD COLUMN home_x REAL; ALTER TABLE spots ADD COLUMN home_y REAL;"),
    M::up("ALTER TABLE nodes ADD COLUMN bookmark INTEGER;"),
    M::up("ALTER TABLE nodes ADD COLUMN wait TEXT;"),
    M::up(
        "CREATE TABLE wait_ids (id INTEGER PRIMARY KEY AUTOINCREMENT);
        UPDATE nodes SET wait = json_set(wait, '$.id', id,
            '$.answerable', json(CASE json_extract(wait, '$.source') WHEN 'ask' THEN 'true' ELSE 'false' END),
            '$.doc', json('null'))
            WHERE wait IS NOT NULL;
        INSERT INTO sqlite_sequence (name, seq) VALUES ('wait_ids', (SELECT ifnull(max(id), 0) FROM nodes));",
    ),
    M::up(
        "CREATE TABLE questions (
            id INTEGER PRIMARY KEY,
            node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            wait TEXT NOT NULL
        );
        CREATE INDEX questions_node ON questions(node);
        INSERT INTO questions (id, node, wait)
            SELECT wait ->> 'id', id, json_remove(wait, '$.source') FROM nodes WHERE wait ->> 'source' = 'ask';
        UPDATE nodes SET wait = NULL WHERE wait ->> 'source' = 'ask';",
    ),
    M::up(
        "ALTER TABLE nodes ADD COLUMN done TEXT;
        UPDATE nodes SET done = json_remove(wait, '$.source'), wait = NULL WHERE wait ->> 'kind' = 'done';",
    ),
    M::up(
        "DELETE FROM messages WHERE recipient IS NULL AND body LIKE 'done: %'
            AND sender IN (SELECT id FROM nodes WHERE parent IS NULL);",
    ),
    M::up(
        "UPDATE spots SET x = home_x, y = home_y WHERE home_x IS NOT NULL AND home_y IS NOT NULL;
        ALTER TABLE spots DROP COLUMN home_x;
        ALTER TABLE spots DROP COLUMN home_y;",
    ),
    M::up(
        "CREATE TABLE devices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            label TEXT NOT NULL,
            token_hash BLOB NOT NULL UNIQUE,
            paired_at INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );",
    ),
    M::up(
        "CREATE TABLE dones (
            id INTEGER PRIMARY KEY,
            node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            wait TEXT NOT NULL
        );
        CREATE INDEX dones_node ON dones(node);
        INSERT INTO dones (id, node, wait) SELECT done ->> 'id', id, done FROM nodes WHERE done IS NOT NULL;
        ALTER TABLE nodes DROP COLUMN done;",
    ),
    M::up("ALTER TABLE nodes DROP COLUMN brief;"),
    M::up(
        "ALTER TABLE nodes ADD COLUMN role TEXT;
        CREATE TABLE gates (
            repo INTEGER NOT NULL,
            branch TEXT NOT NULL,
            owner INTEGER NOT NULL,
            gate INTEGER NOT NULL,
            target TEXT NOT NULL,
            worktree TEXT NOT NULL,
            state TEXT NOT NULL,
            head TEXT NOT NULL,
            merged TEXT,
            run INTEGER NOT NULL DEFAULT 0,
            verdict TEXT,
            opened INTEGER NOT NULL,
            updated INTEGER NOT NULL,
            PRIMARY KEY (repo, branch)
        );",
    ),
    M::up("ALTER TABLE nodes ADD COLUMN card_view TEXT; ALTER TABLE nodes ADD COLUMN reply TEXT;"),
    M::up("ALTER TABLE nodes ADD COLUMN task TEXT;"),
    M::up(
        "ALTER TABLE messages ADD COLUMN what TEXT;
        CREATE TABLE skill_digests (
            skill TEXT PRIMARY KEY,
            digest TEXT NOT NULL
        );",
    ),
    M::up(
        "UPDATE events SET entry = json_set(entry, '$.reason.code', 'kindWord') WHERE entry ->> '$.reason.code' = 'gateWord';
        UPDATE events SET entry = json_set(entry, '$.reason.code', 'git', '$.reason.value', 'Old rule: a node with a parent cannot loom ask the human.') WHERE entry ->> '$.reason.code' = 'hasParent';",
    ),
    M::up(
        "CREATE TABLE answers (id INTEGER PRIMARY KEY);
        INSERT OR IGNORE INTO answers (id)
            SELECT entry ->> 'wait' FROM events WHERE entry ->> 'kind' = 'answered';
        INSERT OR IGNORE INTO answers (id)
            SELECT what ->> '$.question.id' FROM messages
            WHERE what ->> 'kind' = 'answer' AND what ->> '$.question.in' = 'inbox';",
    ),
    M::up(
        "ALTER TABLE nodes ADD COLUMN rules TEXT;
        ALTER TABLE nodes ADD COLUMN asleep INTEGER NOT NULL DEFAULT 0;
        DROP TABLE skill_digests;",
    ),
    M::up(
        r#"ALTER TABLE gates DROP COLUMN run;
        ALTER TABLE nodes ADD COLUMN review TEXT;
        ALTER TABLE gates ADD COLUMN queued INTEGER;
        ALTER TABLE gates ADD COLUMN seq INTEGER;
        ALTER TABLE gates ADD COLUMN batch INTEGER;
        UPDATE gates SET state = '"merging"' WHERE state IN ('"passed"', '"merging"')
            OR (state = '"ready"' AND gate IS NOT coalesce((SELECT p.id FROM nodes o JOIN nodes p ON p.id = o.parent
                WHERE o.id = gates.owner AND p.role = '코디'), owner));
        CREATE TABLE batches (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo INTEGER NOT NULL,
            target TEXT NOT NULL,
            base TEXT,
            tip TEXT,
            started INTEGER NOT NULL
        );
        UPDATE events SET entry = json_set(entry, '$.gate.verb', 'queue') WHERE entry ->> '$.gate.verb' = 'merge';
        UPDATE events SET entry = json_set(entry, '$.gate.findings',
            (SELECT json_group_array(json(value)) FROM json_each(entry, '$.gate.findings') WHERE value ->> '$.kind' != 'targetMoved'))
            WHERE EXISTS (SELECT 1 FROM json_each(entry, '$.gate.findings') WHERE value ->> '$.kind' = 'targetMoved');
        UPDATE events SET entry = json_set(entry, '$.reason.value.verb', 'queue') WHERE entry ->> '$.reason.code' = 'gate' AND entry ->> '$.reason.value.verb' = 'merge';
        UPDATE events SET entry = json_set(entry, '$.reason.value.allowed',
            (SELECT json_group_array(CASE value WHEN 'merge' THEN 'queue' ELSE value END) FROM json_each(entry, '$.reason.value.allowed')))
            WHERE entry ->> '$.reason.code' = 'gate';
        UPDATE events SET entry = json_set(entry, '$.what', json('{"kind":"note"}')) WHERE entry ->> '$.what.kind' = 'merging';
        UPDATE messages SET what = '{"kind":"note"}' WHERE what ->> '$.kind' = 'merging';
        UPDATE events SET entry = json_set(entry, '$.reason', json_object('code', 'git', 'value', CASE entry ->> '$.reason.code'
                WHEN 'notAncestor' THEN 'Old rule: loom ready needed the branch to contain its target.'
                WHEN 'noGate' THEN 'Old rule: a branch outside a coordinator needed one live CI 담당자 node.'
                WHEN 'manyGates' THEN 'Old rule: a branch outside a coordinator needed one live CI 담당자 node.'
                WHEN 'noGateScripts' THEN 'Old rule: loom merge needed the gate scripts.'
                WHEN 'staleRun' THEN 'Old rule: a loom merge report had to come from the current run.'
                ELSE 'Old rule: a loom merge report needed its merge commit in the target.' END))
            WHERE entry ->> '$.reason.code' IN ('notAncestor', 'noGate', 'manyGates', 'noGateScripts', 'staleRun', 'notMerged');"#,
    ),
    M::up(
        r#"UPDATE settings SET value = json_set(settings.value, '$.roles',
            (SELECT json_group_array(json(r.value)) FROM json_each(settings.value, '$.roles') r
                WHERE r.value ->> '$.name' IS NOT 'CI 담당자'))
            WHERE json_type(settings.value, '$.roles') = 'array';
        UPDATE settings SET value = json_insert(settings.value, '$.roles[#]', json_object(
                'name', '고위험 워커',
                'duty', '저장 형식, 인증, 동시성, 데이터 손실 경로, 여러 모듈의 계약을 바꾸는 편집 범위의 구현과 검증. 다시 위임하지 않는다',
                'agent', coalesce((SELECT r.value ->> '$.agent' FROM json_each(settings.value, '$.roles') r
                    WHERE r.value ->> '$.name' = '워커' AND r.value ->> '$.agent' IN ('claude', 'cursor')), 'claude'),
                'model', 'claude-opus-5-5',
                'effort', 'high'))
            WHERE json_type(settings.value, '$.roles') = 'array'
            AND NOT EXISTS (SELECT 1 FROM json_each(settings.value, '$.roles') r WHERE r.value ->> '$.name' = '고위험 워커');
        UPDATE events SET entry = json_set(entry, '$.reason', json_object('code', 'git', 'value', 'Old rule: loom spawn took --agent without --role.'))
            WHERE entry ->> '$.reason.code' = 'noAgentOrRole';"#,
    ),
    M::up(
        r#"ALTER TABLE nodes ADD COLUMN branched_from TEXT;
        UPDATE events SET entry = json_set(entry, '$.reason.code', 'leadChildNeedsWorktree')
            WHERE entry ->> '$.reason.code' = 'leadSpawnsOnlyCoordinator';
        UPDATE gates SET gate = owner WHERE state = '"merging"' AND batch IS NULL;"#,
    ),
    M::up(
        "CREATE TABLE agent_states (
            node INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
            state TEXT NOT NULL,
            seq INTEGER NOT NULL,
            noted INTEGER,
            at INTEGER NOT NULL
        );",
    ),
];
const MIGRATIONS: Migrations = Migrations::from_slice(STEPS);

pub fn default_path() -> std::io::Result<PathBuf> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("loom.db"))
}

#[cfg(windows)]
pub fn data_dir() -> std::io::Result<PathBuf> {
    let appdata =
        std::env::var_os("APPDATA").ok_or_else(|| std::io::Error::other("APPDATA가 없다"))?;
    Ok(PathBuf::from(appdata).join("loom"))
}

#[cfg(not(windows))]
pub fn data_dir() -> std::io::Result<PathBuf> {
    let home =
        std::env::home_dir().ok_or_else(|| std::io::Error::other("홈 폴더를 찾지 못했다"))?;
    Ok(home.join(".loom"))
}

#[derive(Clone)]
pub struct Database(Arc<Mutex<Connection>>);

impl Database {
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap()
    }
}

pub fn open(path: &Path) -> std::io::Result<Database> {
    let mut conn = Connection::open(path).map_err(std::io::Error::other)?;
    conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(std::io::Error::other)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(std::io::Error::other)?;
    match MIGRATIONS.to_latest(&mut conn) {
        Ok(()) => Ok(Database(Arc::new(Mutex::new(conn)))),
        Err(rusqlite_migration::Error::MigrationDefinition(
            MigrationDefinitionError::DatabaseTooFarAhead,
        )) => Err(std::io::Error::other(format!(
            "this loom ({}) is older than the data in {}. Open the newer loom.",
            crate::protocol::VERSION,
            path.display()
        ))),
        Err(e) => Err(std::io::Error::other(e)),
    }
}

pub fn import(db: &Database, from: &Path) -> Result<(), String> {
    let conn = db.lock();
    let used: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM repos) + (SELECT count(*) FROM nodes)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if used > 0 {
        return Err("this loom already has repositories or nodes".into());
    }
    conn.execute("ATTACH DATABASE ?1 AS old", [from.to_string_lossy()])
        .map_err(|e| e.to_string())?;
    let copied = copy_tables(&conn);
    let detached = conn.execute("DETACH DATABASE old", []);
    copied.map_err(|e| e.to_string())?;
    detached.map(|_| ()).map_err(|e| e.to_string())
}

fn copy_tables(conn: &Connection) -> rusqlite::Result<()> {
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM old.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
            ORDER BY name != 'repos', name != 'nodes', name",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let tx = conn.unchecked_transaction()?;
    for table in &tables {
        tx.execute(
            &format!("INSERT OR REPLACE INTO main.\"{table}\" SELECT * FROM old.\"{table}\" ORDER BY rowid"),
            [],
        )?;
    }
    tx.execute_batch(
        "DELETE FROM main.sqlite_sequence;
        INSERT INTO main.sqlite_sequence SELECT * FROM old.sqlite_sequence;",
    )?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn import_keeps_rows_and_numbers_and_refuses_a_used_db() {
        let dir = tempfile::tempdir().unwrap();
        let old = super::open(&dir.path().join("old.db")).unwrap();
        old.lock()
            .execute_batch(
                "INSERT INTO repos (path) VALUES ('/r');
                INSERT INTO nodes (id, name, agent, state, repo, worktree) VALUES (7, 'a', 'null', '{}', 1, '/r');
                INSERT INTO nodes (id, name, agent, parent, state, repo, worktree) VALUES (9, 'b', 'null', 7, '{}', 1, '/r');
                INSERT INTO settings (id, value) VALUES (1, '{\"theme\":\"dark\"}');",
            )
            .unwrap();
        drop(old);
        let new = super::open(&dir.path().join("new.db")).unwrap();
        super::import(&new, &dir.path().join("old.db")).unwrap();
        let conn = new.lock();
        let parent: u32 = conn
            .query_row("SELECT parent FROM nodes WHERE id = 9", [], |r| r.get(0))
            .unwrap();
        assert_eq!(parent, 7);
        conn.execute(
            "INSERT INTO nodes (name, agent, state) VALUES ('c', 'null', '{}')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 10);
        drop(conn);
        let again = super::import(&new, &dir.path().join("old.db"));
        assert_eq!(
            again,
            Err("this loom already has repositories or nodes".into())
        );
    }

    #[test]
    fn newer_data_is_refused_with_a_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = super::open(&path).unwrap();
        let conn = db.lock();
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        conn.pragma_update(None, "user_version", version + 1)
            .unwrap();
        drop(conn);
        drop(db);
        let error = super::open(&path).err().unwrap().to_string();
        assert_eq!(
            error,
            format!(
                "this loom ({}) is older than the data in {}. Open the newer loom.",
                crate::protocol::VERSION,
                path.display()
            )
        );
    }

    #[test]
    fn a_read_then_write_transaction_survives_another_connections_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let first = super::open(&path).unwrap();
        let second = super::open(&path).unwrap();
        let mut first = first.lock();
        let tx = first.transaction().unwrap();
        let _: u32 = tx
            .query_row("SELECT count(*) FROM repos", [], |row| row.get(0))
            .unwrap();
        let other = std::thread::spawn(move || {
            second
                .lock()
                .execute("INSERT INTO repos (path) VALUES ('/b')", [])
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        tx.execute("INSERT INTO repos (path) VALUES ('/a')", [])
            .unwrap();
        tx.commit().unwrap();
        other.join().unwrap().unwrap();
    }

    #[test]
    fn migrations_are_valid() {
        super::MIGRATIONS.validate().unwrap();
    }

    #[test]
    fn step_23_numbers_open_waits_and_continues_after_them() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 22).unwrap();
        conn.execute_batch(
            r#"INSERT INTO nodes (id, name, agent, state, wait) VALUES
            (4, 'a', 'null', '{}', '{"kind":"question","text":"A?","since":1,"source":"ask"}'),
            (9, 'b', 'null', '{}', '{"kind":"trust","text":"","since":1,"source":"screen"}'),
            (10, 'c', 'null', '{}', NULL);"#,
        )
        .unwrap();
        super::MIGRATIONS.to_version(&mut conn, 23).unwrap();
        let waits = conn
            .prepare("SELECT wait ->> 'id', wait ->> 'answerable' FROM nodes WHERE wait IS NOT NULL ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(waits, [(4, true), (9, false)]);
        conn.execute("INSERT INTO wait_ids DEFAULT VALUES", [])
            .unwrap();
        assert_eq!(conn.last_insert_rowid(), 11);
    }

    #[test]
    fn step_24_moves_open_asks_to_questions() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 23).unwrap();
        conn.execute_batch(
            r#"INSERT INTO nodes (id, name, agent, state, wait) VALUES
            (4, 'a', 'null', '{}', '{"id":7,"kind":"question","text":"A?","since":1,"answerable":true,"doc":null,"source":"ask","doc_path":"/m.html"}'),
            (9, 'b', 'null', '{}', '{"id":8,"kind":"trust","text":"","since":1,"answerable":false,"doc":null,"source":"screen"}');"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let moved: (u32, u32, String) = conn
            .query_row("SELECT id, node, wait FROM questions", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(
            moved,
            (7, 4, r#"{"id":7,"kind":"question","text":"A?","since":1,"answerable":true,"doc":null,"doc_path":"/m.html"}"#.into())
        );
        let left: Vec<Option<String>> = conn
            .prepare("SELECT wait ->> 'source' FROM nodes ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(left, [None, Some("screen".into())]);
    }

    #[test]
    fn step_31_adds_node_roles_and_the_gate_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 30).unwrap();
        conn.execute(
            "INSERT INTO nodes (id, name, agent, state) VALUES (4, 'a', 'null', '{}')",
            [],
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let role: Option<String> = conn
            .query_row("SELECT role FROM nodes WHERE id = 4", [], |row| row.get(0))
            .unwrap();
        assert_eq!(role, None);
        let insert = "INSERT INTO gates (repo, branch, owner, gate, target, worktree, state, head, opened, updated)
            VALUES (1, 'b', 4, 4, 'main', '/w', '\"ready\"', 'c', 1, 1)";
        conn.execute(insert, []).unwrap();
        assert!(
            conn.execute(insert, []).is_err(),
            "one row per repo and branch"
        );
    }

    #[test]
    fn step_34_adds_message_kinds() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 33).unwrap();
        conn.execute(
            r#"INSERT INTO messages (recipient, sender, sender_name, kind, body) VALUES (NULL, NULL, 'a', '"queue"', 'done: 끝')"#,
            [],
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let what: Option<String> = conn
            .query_row("SELECT what FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(what, None);
        let digests: u32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'skill_digests'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(digests, 0);
    }

    #[test]
    fn step_36_keeps_the_questions_that_the_human_answered() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 35).unwrap();
        for entry in [
            r#"{"kind":"answered","node":{"id":1,"name":"a"},"wait":7,"text":"A","images":0}"#,
            r#"{"kind":"unanswered","node":{"id":1,"name":"a"},"wait":8,"by":"agent"}"#,
        ] {
            conn.execute("INSERT INTO events (at, entry) VALUES (1, ?1)", [entry])
                .unwrap();
        }
        conn.execute(
            r#"INSERT INTO messages (recipient, sender, sender_name, kind, body, what) VALUES
                (NULL, NULL, 'human', '"queue"', 'a', '{"kind":"answer","question":{"in":"inbox","id":9}}'),
                (NULL, NULL, 'b', '"queue"', 'b', '{"kind":"answer","question":{"in":"message","id":10}}')"#,
            [],
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let ids: Vec<u32> = conn
            .prepare("SELECT id FROM answers ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ids, [7, 9]);
    }

    #[test]
    fn step_35_renames_old_refusal_codes() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 34).unwrap();
        let old = r#"{"kind":"refused","how":"queue","from":{"id":2,"name":"child"},"to":"root","reason":{"code":"gateWord","value":{"word":"READY","verb":"loom ready"}}}"#;
        let has_parent = r#"{"kind":"refused","how":"queue","from":{"id":2,"name":"child"},"to":"root","reason":{"code":"hasParent"}}"#;
        for entry in [old, has_parent] {
            conn.execute("INSERT INTO events (at, entry) VALUES (1, ?1)", [entry])
                .unwrap();
        }
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let reasons: Vec<crate::protocol::Refusal> = conn
            .prepare("SELECT entry ->> '$.reason' FROM events ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|json| serde_json::from_str(&json.unwrap()).unwrap())
            .collect();
        assert_eq!(
            reasons[0],
            crate::protocol::Refusal::KindWord {
                word: "READY".into(),
                verb: "loom ready".into()
            }
        );
        assert!(matches!(reasons[1], crate::protocol::Refusal::Git(_)));
    }

    #[test]
    fn step_39_drops_the_ci_role_adds_the_high_risk_worker_and_renames_the_old_spawn_refusal() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 38).unwrap();
        let role = |name: &str, agent: &str| serde_json::json!({"name": name, "duty": "d", "agent": agent, "model": null, "effort": null});
        let roles = serde_json::json!({"theme": "dark", "roles": [role("워커", "cursor"), role("CI 담당자", "cursor"), role("평가자", "codex")]});
        conn.execute(
            "INSERT INTO settings (id, value) VALUES (1, ?1)",
            [roles.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (at, entry) VALUES (1, ?1)",
            [r#"{"kind":"refused","how":"queue","from":{"id":2,"name":"c"},"to":"x","reason":{"code":"noAgentOrRole"}}"#],
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let value: String = conn
            .query_row("SELECT value FROM settings", [], |row| row.get(0))
            .unwrap();
        let view: crate::settings::SettingsView = serde_json::from_str(&value).unwrap();
        let names: Vec<&str> = view.roles.0.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["워커", "평가자", crate::roles::HIGH_RISK_WORKER]);
        let added = &view.roles.0[2];
        assert_eq!(added.duty, crate::roles::HIGH_RISK_DUTY);
        assert_eq!(added.agent, Some(crate::protocol::AgentId("cursor".into())));
        assert_eq!(added.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(added.effort.as_deref(), Some("high"));
        assert_eq!(
            view.roles.launch_problem(crate::profile::check_choice),
            None
        );
        let reason: String = conn
            .query_row("SELECT entry ->> '$.reason' FROM events", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(matches!(
            serde_json::from_str(&reason).unwrap(),
            crate::protocol::Refusal::Git(_)
        ));
    }

    #[test]
    fn step_40_renames_the_lead_spawn_refusal_and_gives_old_ci_rounds_to_the_owner() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 39).unwrap();
        conn.execute(
            "INSERT INTO events (at, entry) VALUES (1, ?1)",
            [r#"{"kind":"refused","how":"queue","from":{"id":2,"name":"c"},"to":"x","reason":{"code":"leadSpawnsOnlyCoordinator","value":"워커"}}"#],
        )
        .unwrap();
        conn.execute_batch(
            r#"INSERT INTO nodes (id, name, agent, state) VALUES (1, 'lead', 'null', '{}'), (2, 'ci', 'null', '{}');
            INSERT INTO nodes (id, name, agent, state, parent) VALUES (3, 'c', 'null', '{}', 1), (4, 'w', 'null', '{}', 3);
            INSERT INTO gates (repo, branch, owner, gate, target, worktree, state, head, opened, updated, batch) VALUES
            (1, 'old', 3, 2, 'main', '/w', '"merging"', 'c1', 1, 1, NULL),
            (1, 'ci-parent', 4, 3, 'main', '/w', '"merging"', 'c2', 1, 1, NULL),
            (1, 'run', 3, 2, 'main', '/w', '"merging"', 'c3', 1, 1, 7);"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let gates: Vec<(String, i64)> = conn
            .prepare("SELECT branch, gate FROM gates ORDER BY branch")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            gates,
            [
                ("ci-parent".into(), 4),
                ("old".into(), 3),
                ("run".into(), 2)
            ]
        );
        let reason: String = conn
            .query_row("SELECT entry ->> '$.reason' FROM events", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::protocol::Refusal>(&reason).unwrap(),
            crate::protocol::Refusal::LeadChildNeedsWorktree("워커".into())
        );
    }

    #[test]
    fn step_38_keeps_old_merge_records_readable_and_queues_nothing() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 37).unwrap();
        let from = r#""from":{"id":2,"name":"ci"}"#;
        let trace = |verb: &str, findings: &str| {
            format!(
                r#""gate":{{"verb":"{verb}","branch":"b","from":"passed","to":"merging","role":null,"agent":null,"model":null,"findings":{findings}}}"#
            )
        };
        let rows = [
            format!(
                r#"{{"kind":"message","how":"queue",{from},"to":{{"id":3,"name":"w"}},"step":"sent","what":{{"kind":"merging","branch":"b","head":"c0ffee1"}},{}}}"#,
                trace(
                    "merge",
                    r#"[{"kind":"targetMoved","reviewed":"a","now":"b"},{"kind":"headMoved","reviewed":"a","now":"b"}]"#
                )
            ),
            format!(
                r#"{{"kind":"refused","how":"queue",{from},"to":"b","reason":{{"code":"gate","value":{{"state":"working","verb":"merge","allowed":["ready","merge"]}}}},{}}}"#,
                trace("merge", "[]")
            ),
            format!(
                r#"{{"kind":"refused","how":"queue",{from},"to":"b","reason":{{"code":"notAncestor","value":{{"branch":"b","target":"main"}}}}}}"#
            ),
            format!(
                r#"{{"kind":"refused","how":"queue",{from},"to":"b","reason":{{"code":"noGate"}}}}"#
            ),
        ];
        for entry in &rows {
            conn.execute("INSERT INTO events (at, entry) VALUES (1, ?1)", [entry])
                .unwrap();
        }
        conn.execute_batch(
            r#"INSERT INTO nodes (id, name, agent, state, role) VALUES (1, 'w', 'null', '{}', NULL), (2, 'ci', 'null', '{}', 'CI 담당자'),
                (5, '코디-1', 'null', '{}', '코디');
            INSERT INTO nodes (id, name, agent, state, parent) VALUES (6, 'cw', 'null', '{}', 5);
            INSERT INTO gates (repo, branch, owner, gate, target, worktree, state, head, opened, updated) VALUES
            (1, 'p', 1, 2, 'main', '/w', '"passed"', 'c1', 1, 1),
            (1, 'm', 1, 2, 'main', '/w', '"merging"', 'c2', 1, 1),
            (1, 'r', 1, 2, 'main', '/w', '"ready"', 'c3', 1, 1),
            (1, 's', 1, 1, 'main', '/w', '"ready"', 'c4', 1, 1),
            (1, 't', 6, 5, 'c', '/c', '"ready"', 'c5', 1, 1);"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let entries: Vec<crate::graph::Entry> = conn
            .prepare("SELECT entry FROM events ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|json| serde_json::from_str(&json.unwrap()).unwrap())
            .collect();
        use crate::graph::Entry;
        use crate::protocol::{Finding, GateVerb, MessageKind, Refusal};
        let Entry::Message {
            what: Some(MessageKind::Note),
            gate: Some(trace),
            ..
        } = &entries[0]
        else {
            panic!("{:?}", entries[0]);
        };
        assert_eq!(trace.verb, GateVerb::Queue);
        assert!(matches!(trace.findings[..], [Finding::HeadMoved { .. }]));
        let Entry::Refused {
            reason: Refusal::Gate(e),
            ..
        } = &entries[1]
        else {
            panic!("{:?}", entries[1]);
        };
        assert_eq!(
            (e.verb, &e.allowed[..]),
            (GateVerb::Queue, &[GateVerb::Ready, GateVerb::Queue][..])
        );
        for entry in &entries[2..] {
            assert!(
                matches!(
                    entry,
                    Entry::Refused {
                        reason: Refusal::Git(_),
                        ..
                    }
                ),
                "{entry:?}"
            );
        }
        let states: Vec<String> = conn
            .prepare("SELECT state FROM gates ORDER BY branch")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            states,
            [
                r#""merging""#,
                r#""merging""#,
                r#""merging""#,
                r#""ready""#,
                r#""ready""#
            ]
        );
    }

    #[test]
    fn step_29_moves_done_notices_to_dones() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 28).unwrap();
        let done = r#"{"id":7,"kind":"done","text":"끝","since":1,"answerable":true,"doc":null,"instruction":"일해","images":[]}"#;
        conn.execute(
            "INSERT INTO nodes (id, name, agent, state, done) VALUES (4, 'a', 'null', '{}', ?1), (9, 'b', 'null', '{}', NULL)",
            [done],
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let moved: Vec<(u32, u32, String)> = conn
            .prepare("SELECT id, node, wait FROM dones")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(moved, [(7, 4, done.to_owned())]);
    }

    #[test]
    fn step_19_drops_worktree_spots_and_keeps_card_spots() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 18).unwrap();
        conn.execute_batch(
            r#"INSERT INTO spots (spot, repo, x, y) VALUES
            ('{"kind":"worktree","repo":1,"path":"/w"}', 1, 0, 0),
            ('{"kind":"node","repo":1,"node":7}', 1, 0, 0),
            ('{"kind":"repo","repo":1}', 1, 0, 0);
            INSERT INTO arranged_repos (repo) VALUES (1);"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let kinds = conn
            .prepare("SELECT spot ->> 'kind' FROM spots ORDER BY 1")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(kinds, ["node", "repo"]);
        let arranged: i64 = conn
            .query_row("SELECT count(*) FROM arranged_repos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(arranged, 1);
    }

    #[test]
    fn step_14_preserves_completion_and_marks_only_exited_processes() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 13).unwrap();
        conn.execute_batch(
            r#"INSERT INTO nodes(name, agent, state) VALUES
            ('running', 'null', '{"kind":"running"}'),
            ('done', 'null', '{"kind":"done","summary":"finished"}'),
            ('exited', 'null', '{"kind":"exited","code":0}');"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let states = conn
            .prepare("SELECT state, process_exited FROM nodes ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            states,
            vec![
                (r#"{"kind":"running"}"#.into(), false),
                (r#"{"kind":"done","summary":"finished"}"#.into(), false),
                (r#"{"kind":"exited","code":0}"#.into(), true),
            ]
        );
    }

    #[test]
    fn step_2_keeps_agent_profiles_and_clears_command() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 1).unwrap();
        conn.execute_batch(
            r#"INSERT INTO nodes (name, profile, state) VALUES ('a', '"claude"', '{}'), ('b', '"command"', '{}');"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let agents: Vec<String> = conn
            .prepare("SELECT agent FROM nodes ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(agents, [r#""claude""#, "null"]);
    }

    #[test]
    fn step_8_removes_default_and_disabled_agents_from_settings() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 6).unwrap();
        conn.execute_batch(
            r#"INSERT INTO settings (id, value) VALUES (1, '{"defaultAgent":"codex","disabledAgents":["aider"],"theme":"dark"}');"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let value: String = conn
            .query_row("SELECT value FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, r#"{"theme":"dark"}"#);
    }

    #[test]
    fn step_12_renames_card_views() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 11).unwrap();
        conn.execute_batch(r#"INSERT INTO settings (id, value) VALUES (1, '{"cardView":"header","theme":"dark"}');"#)
            .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let value: String = conn
            .query_row("SELECT value FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, r#"{"cardView":"simple","theme":"dark"}"#);
    }

    #[test]
    fn step_5_keeps_messages_with_sender_names() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 4).unwrap();
        conn.execute_batch(
            r#"INSERT INTO nodes (name, agent, state) VALUES ('총괄', 'null', '{}'), ('a', 'null', '{}');
            INSERT INTO messages (recipient, sender, body, kind) VALUES (1, 2, '질문', '"steer"');"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let row: (u32, u32, String, String, String) = conn
            .query_row(
                "SELECT recipient, sender, sender_name, kind, body FROM messages",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row, (1, 2, "a".into(), r#""steer""#.into(), "질문".into()));
    }

    #[test]
    fn pushed_spots_go_back_to_their_home_and_the_home_columns_go() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::MIGRATIONS.to_version(&mut conn, 26).unwrap();
        conn.execute_batch(
            r#"INSERT INTO spots (spot, repo, x, y, home_x, home_y) VALUES
            ('{"kind":"repo","repo":71}', 71, 1448, 126, 768, 126),
            ('{"kind":"repo","repo":9}', 9, 32, 126, NULL, NULL);"#,
        )
        .unwrap();
        super::MIGRATIONS.to_latest(&mut conn).unwrap();
        let spots = conn
            .prepare("SELECT repo, x, y FROM spots ORDER BY repo")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<(u32, f64, f64)>>>()
            .unwrap();
        assert_eq!(spots, [(9, 32.0, 126.0), (71, 768.0, 126.0)]);
        assert!(conn.prepare("SELECT home_x FROM spots").is_err());
    }
}
