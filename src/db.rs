//! 기반: SQLite 파일 하나를 연다. 스키마 변경 목록은 이 파일에만 둔다.
//! 기능 모듈이 테이블을 더하려면 `STEPS` 끝에 항목 하나를 더한다. 이미 있는 항목은 고치지 않는다.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use rusqlite_migration::{M, MigrationDefinitionError, Migrations};

const STEPS: &[M] = &[
    // 1: 노드 나무와 메일함(graph). `profile`과 `state`는 serde JSON 문자열이다.
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
    // 2: 노드에 프로필 대신 카탈로그 에이전트 id를 남긴다(graph). 직접 입력은 JSON `null`이다.
    M::up(
        "ALTER TABLE nodes RENAME COLUMN profile TO agent;
        UPDATE nodes SET agent = 'null' WHERE agent = '\"command\"';",
    ),
    // 3: 설정 한 줄(settings). `value`는 serde JSON 문자열이다.
    M::up(
        "CREATE TABLE settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            value TEXT NOT NULL
        );",
    ),
    // 4: 메시지 전달 방식(graph). serde JSON 문자열이다. 앞 메시지는 모두 큐다.
    M::up(r#"ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT '"queue"';"#),
    // 5: 노드 제거와 입양(graph). 보낸 노드 이름을 남긴다. 보낸 노드를 지워도 메시지를 읽을 수 있어야 한다.
    // `recipient`가 NULL이면 부모 없는 `sender`가 보관한 메시지다. 새 부모가 생기면 그 부모가 받는다.
    // 받은 메시지는 받는 노드와 함께 지운다.
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
    // 6: 받음 확인(graph). `sent_at`은 꺼낸 시각(유닉스 초)이다. 받는 쪽이 확인하면 `delivered`가 1이 된다.
    M::up("ALTER TABLE messages ADD COLUMN sent_at INTEGER;"),
    // 7: 이벤트 기록(graph). `entry`는 serde JSON 문자열이다. `message`는 메시지 이벤트가 가리키는 메시지다.
    // 메시지를 지워도 기록은 남으므로 외래 키를 두지 않는다. id는 1씩 늘어난다(AUTOINCREMENT 없이 max+1).
    M::up(
        "CREATE TABLE events (
            id INTEGER PRIMARY KEY,
            at INTEGER NOT NULL,
            entry TEXT NOT NULL,
            message INTEGER
        );",
    ),
    // 8: 설정에서 기본 에이전트와 에이전트 켜기·끄기를 지웠다(settings). 새 노드 창의 「최근」과 검색 목록이 맡는다.
    M::up("UPDATE settings SET value = json_remove(value, '$.defaultAgent', '$.disabledAgents');"),
    // 9: 노드의 모델과 추론 강도(graph). CLI가 알린 글 그대로다. 알리지 않았으면 NULL이다.
    M::up("ALTER TABLE nodes ADD COLUMN model TEXT; ALTER TABLE nodes ADD COLUMN effort TEXT;"),
    // 10: 저장소 목록(worktree)과 노드의 자리(graph). `path`는 저장소 원래 체크아웃의 절대 경로다.
    // 워크트리 묶음은 `git worktree list`가 원천이라 표를 두지 않는다. 노드는 띄운 워크트리 경로를 남긴다.
    // 옛 노드는 `repo`가 NULL이다. 데몬이 시작할 때 시작 폴더 저장소로 옮긴다(`Graph::settle`).
    M::up(
        "CREATE TABLE repos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE
        );
        ALTER TABLE nodes ADD COLUMN repo INTEGER;
        ALTER TABLE nodes ADD COLUMN worktree TEXT;",
    ),
    // 11: 노드의 첫 지시 첫 줄(graph). 카드 주제에 쓴다. 첫 지시 없이 만든 노드와 옛 노드는 NULL이다.
    M::up("ALTER TABLE nodes ADD COLUMN brief TEXT;"),
    // 12: 카드 보기 이름을 바꿨다(settings). 「터미널」은 「표준」, 「머리만」은 「심플」이다.
    M::up(
        "UPDATE settings SET value = json_set(value, '$.cardView',
            CASE json_extract(value, '$.cardView') WHEN 'terminal' THEN 'standard' WHEN 'header' THEN 'simple' ELSE json_extract(value, '$.cardView') END)
        WHERE json_extract(value, '$.cardView') IS NOT NULL;",
    ),
    // 13: 노드가 데몬을 부를 때 내는 토큰과, 노드 서버에 다시 붙는 값(graph). 노드 프로세스는 데몬이 다시 시작해도 살므로
    // 새 데몬도 같은 토큰을 받아야 한다. `server`는 serde JSON 문자열(`ServerLink`)이고 서버가 없는 노드는 NULL이다.
    M::up("ALTER TABLE nodes ADD COLUMN token TEXT; ALTER TABLE nodes ADD COLUMN server TEXT;"),
    // 14: 작업 완료와 프로세스 종료를 나눈다. 답변은 원래 메시지 번호로 연결한다.
    M::up(
        "ALTER TABLE nodes ADD COLUMN process_exited INTEGER NOT NULL DEFAULT 0;
        UPDATE nodes SET process_exited = 1 WHERE state ->> 'kind' = 'exited';
        ALTER TABLE messages ADD COLUMN reply_to INTEGER;
        CREATE INDEX messages_replies ON messages(recipient, reply_to) WHERE reply_to IS NOT NULL;",
    ),
    // 15: 사람이 끌어 놓은 자리(spots). `spot`은 serde JSON 문자열(`Spot`)이다. 저장소와 노드를 지우면 같은 트랜잭션에서 자리도 지운다.
    // 저장소·노드 번호는 AUTOINCREMENT라 다시 쓰지 않는다. 워크트리는 표가 없어 지울 때 데몬이 자리를 지운다(`Spots::forget_worktree`).
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
    // 16: 사람이 바꾼 표준 카드 크기(목업 docs/mockups/card-resize.html). [정렬]은 자리만 지우고 크기는 지킨다. 노드를 지우면 크기도 지운다.
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
    // 17: 정렬한 모양을 지키는 저장소(spots). [정렬]이 넣고, 사람이 그 저장소의 묶음·카드를 끌어 놓으면 뺀다.
    // 화면을 닫은 동안 생긴 카드·묶음도 다음에 열 때 정렬한 모양으로 놓는다. 저장소를 지우면 함께 지운다.
    M::up(
        "CREATE TABLE arranged_repos (repo INTEGER PRIMARY KEY);
        CREATE TRIGGER arranged_of_repo AFTER DELETE ON repos BEGIN
            DELETE FROM arranged_repos WHERE repo = OLD.id;
        END;",
    ),
    // 18: 계정 전환(graph). `account`는 노드를 띄운 계정(serde JSON `AccountId`)이고 시스템 기본·옛 노드는 NULL이다.
    // `transcript`는 Claude 노드 세션의 기록 파일 경로다. 세션을 시작하기 전과 다른 프로필은 NULL이다.
    M::up("ALTER TABLE nodes ADD COLUMN account TEXT; ALTER TABLE nodes ADD COLUMN transcript TEXT;"),
    // 19: 자식 워크트리 묶음은 부모 카드의 묶음 안에 선다(목업 docs/mockups/arrange-family.html v4). 묶음 자리는 바깥 상자(저장소 상자나 부모 묶음) 안 좌표다.
    // 앞 묶음 자리는 모두 저장소 기준이라 지운다. 카드 자리와 정렬한 저장소는 남긴다. 정렬한 저장소는 화면이 새 자동 배치를 저장한다.
    // 바깥 상자는 다른 워크트리 부모를 가진 노드가 정한다(web/src/repos.ts `groupParents`). 그 노드가 생기거나 부모가 바뀌면 그 묶음 자리를 지운다.
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
    // 20: 끌지 않고 커진 상자가 민 상자의 밀기 전 자리(spots). 민 상자가 줄거나 사라지면 화면이 그 자리 쪽으로 되돌린다. 사람이 옮기면 NULL이다.
    M::up("ALTER TABLE spots ADD COLUMN home_x REAL; ALTER TABLE spots ADD COLUMN home_y REAL;"),
    // 21: 북마크 차례(graph). 북마크하지 않은 노드는 NULL이다. 노드를 지우면 줄과 함께 사라진다.
    M::up("ALTER TABLE nodes ADD COLUMN bookmark INTEGER;"),
    // 22: 사람 응답 대기(graph `waits`). serde JSON `Stored`(종류, 한 줄, 시작 시각, 신호)이고 대기가 없으면 NULL이다.
    M::up("ALTER TABLE nodes ADD COLUMN wait TEXT;"),
    // 23: 대기 번호(graph `waits`, 사람이 `Q12`로 부른다). `wait_ids`는 번호만 내주고 줄은 바로 지운다. AUTOINCREMENT라 번호를 다시 쓰지 않는다.
    // 이미 열린 대기는 노드 번호를 대기 번호로 받고, 다음 번호는 그 뒤부터다. `loom ask` 대기는 화면에서 답할 수 있다.
    M::up(
        "CREATE TABLE wait_ids (id INTEGER PRIMARY KEY AUTOINCREMENT);
        UPDATE nodes SET wait = json_set(wait, '$.id', id,
            '$.answerable', json(CASE json_extract(wait, '$.source') WHEN 'ask' THEN 'true' ELSE 'false' END),
            '$.doc', json('null'))
            WHERE wait IS NOT NULL;
        INSERT INTO sqlite_sequence (name, seq) VALUES ('wait_ids', (SELECT ifnull(max(id), 0) FROM nodes));",
    ),
    // 24: 노드마다 여러 `loom ask` 질문(graph `waits`, 목업 M28). `wait`은 serde JSON `Asked`이고 `id`는 그 질문 번호다(`wait_ids`에서 받는다).
    // 열린 `loom ask` 대기는 `nodes.wait`에서 이 표로 옮긴다. `nodes.wait`에는 훅·Codex·화면 신호의 대기만 남는다.
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
    // 25: 완료 알림(graph `waits`)의 자리. serde JSON `HumanWait`이고 없으면 NULL이다. 사람이 [확인]을 누를 때만 지운다.
    // 신호의 대기(`nodes.wait`)와 자리가 같으면 승인 창이 완료를 덮거나 완료가 다음 완료를 막는다(2026-10-22 사람 지시 "완료 알림은 사용자가 확인 버튼 클릭하면 제거되는 걸로").
    M::up(
        "ALTER TABLE nodes ADD COLUMN done TEXT;
        UPDATE nodes SET done = json_remove(wait, '$.source'), wait = NULL WHERE wait ->> 'kind' = 'done';",
    ),
    // 26: 부모 없는 노드의 완료 보고는 받은편지함에만 둔다(graph `done`). 전에 보관한 사본을 지운다. 트리에 「보관 1」만 남았다(2026-10-22 사람 지시 "문제 해결해").
    M::up(
        "DELETE FROM messages WHERE recipient IS NULL AND body LIKE 'done: %'
            AND sender IN (SELECT id FROM nodes WHERE parent IS NULL);",
    ),
    // 27: 밀린 상자의 자리는 저장하지 않는다. 화면이 렌더마다 겹친 상자를 민다. 밀린 상자는 밀기 전 자리(`home_x`·`home_y`)로 돌아가 저장한다.
    // 화면이 민 자리를 저장하면 카드를 다르게 잰 두 탭이 번갈아 저장했다(2026-10-22 사람 지시 "state must be one").
    M::up(
        "UPDATE spots SET x = home_x, y = home_y WHERE home_x IS NOT NULL AND home_y IS NOT NULL;
        ALTER TABLE spots DROP COLUMN home_x;
        ALTER TABLE spots DROP COLUMN home_y;",
    ),
    // 28: 짝지은 폰(devices). `token_hash`는 쿠키 토큰의 SHA-256이다. 시각은 Unix 밀리초다.
    M::up(
        "CREATE TABLE devices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            label TEXT NOT NULL,
            token_hash BLOB NOT NULL UNIQUE,
            paired_at INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );",
    ),
    // 29: 노드마다 여러 완료 알림(graph `waits`). `wait`은 serde JSON `HumanWait`이고 `id`는 그 완료 번호다. `nodes.done` 자리 하나는 다음 `loom done`이 확인하지 않은 완료를 덮었다.
    // 근거: 2026-10-22 사람 보고 "완료 알림이 확인 안눌러도 없어지는 버그가 있음"(Q368을 Q370이, Q372를 Q373이 덮었다).
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
];
const MIGRATIONS: Migrations = Migrations::from_slice(STEPS);

/// `<데이터 폴더>/loom.db`. 근거: docs/design.md 「저장」.
pub fn default_path() -> std::io::Result<PathBuf> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("loom.db"))
}

/// Windows는 앱 설정 관례(`%APPDATA%\loom`), 다른 OS는 `~/.loom`이다.
#[cfg(windows)]
fn data_dir() -> std::io::Result<PathBuf> {
    let appdata =
        std::env::var_os("APPDATA").ok_or_else(|| std::io::Error::other("APPDATA가 없다"))?;
    Ok(PathBuf::from(appdata).join("loom"))
}

#[cfg(not(windows))]
fn data_dir() -> std::io::Result<PathBuf> {
    let home =
        std::env::home_dir().ok_or_else(|| std::io::Error::other("홈 폴더를 찾지 못했다"))?;
    Ok(home.join(".loom"))
}

/// 데몬의 DB 연결 하나. 그래프·저장소·설정·카드 자리가 복사본을 나눠 쥐고, 이 잠금 하나로 차례를 정한다.
/// 연결이 여럿이면 SQLite가 연결 사이 차례를 정한다. 쓰기를 기다리는 연결은 잠깐 자고 다시 보기를 되풀이하므로, 다른 연결이 짧은 커밋을 이어 가면 계속 놓친다.
/// 커밋이 20~120 ms 걸리는 바쁜 WSL에서 기다리던 쓰기가 5초 뒤 `database is locked`로 실패했다(2026-10-22 사람 데몬, 근거: docs/design.md 「저장」).
#[derive(Clone)]
pub struct Database(Arc<Mutex<Connection>>);

impl Database {
    /// 쥔 동안 다른 모듈의 DB 함수를 부르지 않는다. 같은 잠금을 다시 잡아 멈춘다.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap()
    }
}

/// 파일을 WAL 모드로 열고 스키마를 최신으로 올린다. 데몬은 한 번만 연다.
/// 트랜잭션은 시작할 때 쓰기 잠금을 잡는다(IMMEDIATE). 기본(DEFERRED)은 읽은 뒤 다른 프로세스가 커밋하면 첫 쓰기가 기다리지 않고 `database is locked`(SQLITE_BUSY_SNAPSHOT)로 실패한다.
/// 더 새 판의 loom이 올린 DB는 열지 않는다. 옛 판은 모르는 표·열을 깨뜨린다.
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
            crate::VERSION,
            path.display()
        ))),
        Err(e) => Err(std::io::Error::other(e)),
    }
}

/// `from`의 모든 행을 이 DB로 옮긴다. `from`은 같은 스키마 판이다(`open`으로 올린 파일). 행 번호를 그대로 둔다.
/// 번호가 겹치지 않게 저장소나 노드가 있는 DB에는 옮기지 않는다.
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

/// 외래 키가 가리키는 표(저장소, 노드)를 먼저 넣는다. 노드는 부모가 먼저 오게 번호 순서다.
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
                crate::VERSION,
                path.display()
            )
        );
    }

    /// 한 연결이 읽고 쓰는 트랜잭션 사이에 다른 프로세스가 커밋해도 쓰기가 `database is locked`로 실패하지 않는다.
    /// 2026-10-22 사람 데몬에서 [답하기]가 이렇게 실패했다. 그때는 모듈마다 연결이 따로였다.
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

    /// 열린 대기는 노드 번호를 대기 번호로 받고, 새 번호는 그 뒤부터다. `loom ask` 대기만 화면에서 답할 수 있다.
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

    /// 열린 `loom ask` 대기는 질문 표로 옮기고, 신호의 대기는 노드에 남긴다.
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

    /// 노드 자리의 완료는 완료 표로 옮긴다. 번호와 내용은 그대로다.
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

    /// 앞 묶음 자리는 저장소 기준이라 지운다. 카드 자리와 정렬한 저장소는 남긴다.
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
        // 6에서 시작해 7(events)과 8을 차례로 올린다.
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

    /// 밀린 상자는 밀기 전 자리로 돌아간다. 화면이 렌더마다 다시 민다. 밀리지 않은 상자는 그대로다.
    #[test]
    fn pushed_spots_go_back_to_their_home_and_the_home_columns_go() {
        let mut conn = Connection::open_in_memory().unwrap();
        // 26단계까지 올린다. 27단계가 밀린 자리를 되돌린다.
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
