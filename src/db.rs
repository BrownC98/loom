//! 기반: SQLite 파일 하나를 연다. 스키마 변경 목록은 이 파일에만 둔다.
//! 기능 모듈이 테이블을 더하려면 `STEPS` 끝에 항목 하나를 더한다. 이미 있는 항목은 고치지 않는다.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

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

/// 파일을 WAL 모드로 열고 스키마를 최신으로 올린다.
/// 연결마다 rusqlite가 busy timeout 5초를 건다(`Connection::open`). `Repos`·`Graph`·`Settings`는 같은 파일에 따로 연결하므로, 다른 연결이 쓰는 동안 바로 `database is locked`로 실패하지 않고 기다린다.
pub fn open(path: &Path) -> Result<Connection, rusqlite_migration::Error> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    MIGRATIONS.to_latest(&mut conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migrations_are_valid() {
        super::MIGRATIONS.validate().unwrap();
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
}
