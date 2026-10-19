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
}
