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
    M::up(
        "ALTER TABLE agent_states DROP COLUMN noted;
        ALTER TABLE messages ADD COLUMN printed INTEGER NOT NULL DEFAULT 0;
        UPDATE messages SET sent_at = NULL WHERE delivered = 0;
        ALTER TABLE nodes ADD COLUMN limited INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE relaunch_plans (
            node INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
            phase TEXT NOT NULL,
            plan TEXT NOT NULL
        );",
    ),
    M::up(
        "ALTER TABLE agent_states ADD COLUMN changed_ns INTEGER NOT NULL DEFAULT 0;
        UPDATE agent_states SET changed_ns = (at + 1) * 1000000000;
        ALTER TABLE agent_states DROP COLUMN at;",
    ),
    M::up(
        "ALTER TABLE nodes ADD COLUMN drafted INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE nodes ADD COLUMN sleeping INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE nodes ADD COLUMN turn_flags INTEGER NOT NULL DEFAULT 0;",
    ),
    M::up("ALTER TABLE nodes DROP COLUMN branched_from;"),
    M::up(
        r#"ALTER TABLE nodes ADD COLUMN process TEXT;
        UPDATE nodes SET process = CASE
            WHEN state ->> 'kind' = 'exited' AND state ->> '$.code' IS NOT NULL
                THEN json_object('kind', 'exited', 'code', state ->> '$.code')
            WHEN state ->> 'kind' = 'exited' THEN '{"kind":"lost"}'
            WHEN state ->> 'kind' = 'setupFailed' THEN json_object('kind', 'setupFailed', 'code', state ->> '$.code')
            WHEN asleep = 1 THEN '{"kind":"asleep"}'
            WHEN process_exited = 1 THEN '{"kind":"lost"}'
        END;
        UPDATE nodes SET state = '{"kind":"running"}' WHERE state ->> 'kind' IN ('exited', 'setupFailed');
        ALTER TABLE nodes DROP COLUMN process_exited;
        ALTER TABLE nodes DROP COLUMN asleep;
        DELETE FROM agent_states WHERE state ->> 'kind' IN ('asleep', 'ended')
            OR node IN (SELECT id FROM nodes WHERE process IS NOT NULL);
        INSERT INTO agent_states (node, state, seq, changed_ns)
            SELECT id, '{"kind":"starting"}', 0, unixepoch() * 1000000000 FROM nodes
            WHERE process IS NULL AND id NOT IN (SELECT node FROM agent_states);
        UPDATE events SET entry = json_set(entry, '$.from', json('null'))
            WHERE entry ->> 'kind' = 'agentStateChanged' AND entry ->> '$.from.kind' IN ('asleep', 'ended');
        UPDATE events SET entry = json_set(entry, '$.to', json('null'))
            WHERE entry ->> 'kind' = 'agentStateChanged' AND entry ->> '$.to.kind' IN ('asleep', 'ended');
        DELETE FROM events
            WHERE entry ->> 'kind' = 'agentStateDisagreed' AND entry ->> '$.state.kind' IN ('asleep', 'ended');"#,
    ),
    M::up("ALTER TABLE nodes ADD COLUMN setup INTEGER NOT NULL DEFAULT 0;"),
    M::up(
        r#"UPDATE events SET entry = json_set(entry, '$.reason', json_object('code', 'git', 'value',
            'Old rule: steer, interrupt, loom rm and loom done --keep went only to a parent or child: ' || (entry ->> '$.reason.value')))
            WHERE entry ->> '$.reason.code' IN ('notRelated', 'notChild');"#,
    ),
    M::up(
        "UPDATE events SET entry = json_set(entry, '$.reason', json_object('code', 'git', 'value', 'Old rule: loom spawn required brief heads.'))
            WHERE entry ->> '$.reason.code' = 'missingHeads';",
    ),
    M::up(
        "CREATE TABLE node_hosts (
            node INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
            address TEXT NOT NULL,
            unreached TEXT
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
