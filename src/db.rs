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
];
const MIGRATIONS: Migrations = Migrations::from_slice(STEPS);

/// `~/.loom/loom.db`. 근거: docs/design.md 「저장」.
pub fn default_path() -> std::io::Result<PathBuf> {
    let home =
        std::env::home_dir().ok_or_else(|| std::io::Error::other("홈 폴더를 찾지 못했다"))?;
    let dir = home.join(".loom");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("loom.db"))
}

/// 파일을 WAL 모드로 열고 스키마를 최신으로 올린다.
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
