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
    #[test]
    fn migrations_are_valid() {
        super::MIGRATIONS.validate().unwrap();
    }
}
