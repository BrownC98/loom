use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::Database;
use crate::graph::NodeId;
use crate::worktree::RepoId;

/// 자리를 가진 상자. 자리의 기준은 상자마다 다르다.
/// 저장소 상자는 그래프 좌표, 맨 위 묶음은 저장소 상자 안 좌표, 자식 묶음과 카드는 묶음 안 좌표다. 그래서 상자를 옮겨도 안의 자리는 그대로다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Spot {
    Repo {
        repo: RepoId,
    },
    /// `path`는 `git worktree list`가 적는 워크트리 절대 경로다(노드의 `place.worktree`와 같은 글자).
    Worktree {
        repo: RepoId,
        path: String,
    },
    Node {
        repo: RepoId,
        node: NodeId,
    },
}

impl Spot {
    fn repo(&self) -> RepoId {
        match self {
            Spot::Repo { repo } | Spot::Worktree { repo, .. } | Spot::Node { repo, .. } => *repo,
        }
    }

    fn inside(&self) -> Option<RepoId> {
        match self {
            Spot::Repo { .. } => None,
            Spot::Worktree { repo, .. } | Spot::Node { repo, .. } => Some(*repo),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Placed {
    pub spot: Spot,
    pub at: Point,
}

/// 저장한 자리 모두와 정렬한 모양을 지키는 저장소.
#[derive(Clone, PartialEq, Debug, Serialize, TS)]
#[ts(export)]
pub struct Layout {
    pub placed: Vec<Placed>,
    pub arranged: Vec<RepoId>,
}

/// [정렬]이나 정렬한 저장소의 새 자동 배치. `repos`는 정렬한 모양을 지킬 저장소다.
#[derive(Clone, PartialEq, Debug, Deserialize, TS)]
#[ts(export)]
pub struct Arranged {
    pub repos: Vec<RepoId>,
    pub placed: Vec<Placed>,
}

/// 사람이 바꾼 표준 카드 크기(그래프 px). 저장한 크기가 없는 카드는 기본 크기다.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CardSize {
    pub node: NodeId,
    pub width: f64,
    pub height: f64,
}

pub struct Spots {
    db: Database,
}

impl Spots {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub fn layout(&self) -> rusqlite::Result<Layout> {
        let db = self.db.lock();
        let arranged = db
            .prepare("SELECT repo FROM arranged_repos")?
            .query_map([], |row| Ok(RepoId(row.get(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(db);
        Ok(Layout {
            placed: self.list()?,
            arranged,
        })
    }

    fn list(&self) -> rusqlite::Result<Vec<Placed>> {
        let db = self.db.lock();
        let mut stmt = db.prepare("SELECT spot, x, y FROM spots")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (spot, x, y) = row?;
            if let Ok(spot) = serde_json::from_str(&spot) {
                out.push(Placed {
                    spot,
                    at: Point { x, y },
                });
            }
        }
        Ok(out)
    }

    pub fn put(&self, placed: &[Placed]) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        write(&tx, placed)?;
        for repo in placed.iter().filter_map(|p| p.spot.inside()) {
            tx.execute("DELETE FROM arranged_repos WHERE repo = ?1", [repo.0])?;
        }
        tx.commit()
    }

    pub fn put_arranged(&self, arranged: &Arranged) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        write(&tx, &arranged.placed)?;
        for repo in &arranged.repos {
            tx.execute(
                "INSERT OR IGNORE INTO arranged_repos (repo) VALUES (?1)",
                [repo.0],
            )?;
        }
        tx.commit()
    }

    pub fn forget_worktree(&self, repo: RepoId, path: &str) -> rusqlite::Result<()> {
        let key = serde_json::to_string(&Spot::Worktree {
            repo,
            path: path.into(),
        })
        .expect("자리는 늘 직렬화된다");
        self.db
            .lock()
            .execute("DELETE FROM spots WHERE spot = ?1", [key])?;
        Ok(())
    }

    pub fn sizes(&self) -> rusqlite::Result<Vec<CardSize>> {
        let db = self.db.lock();
        let mut stmt = db.prepare("SELECT node, width, height FROM card_sizes")?;
        stmt.query_map([], |row| {
            Ok(CardSize {
                node: NodeId(row.get(0)?),
                width: row.get(1)?,
                height: row.get(2)?,
            })
        })?
        .collect()
    }

    pub fn put_sizes(&self, sizes: &[CardSize]) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        for s in sizes {
            tx.execute(
                "INSERT OR REPLACE INTO card_sizes (node, width, height) VALUES (?1, ?2, ?3)",
                rusqlite::params![s.node.0, s.width, s.height],
            )?;
        }
        tx.commit()
    }

    pub fn clear_sizes(&self, nodes: &[NodeId]) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        for node in nodes {
            tx.execute("DELETE FROM card_sizes WHERE node = ?1", [node.0])?;
        }
        tx.commit()
    }

    pub fn clear(&self, repo: RepoId) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM spots WHERE repo = ?1", [repo.0])?;
        tx.execute("DELETE FROM arranged_repos WHERE repo = ?1", [repo.0])?;
        tx.commit()
    }
}

fn write(tx: &rusqlite::Transaction, placed: &[Placed]) -> rusqlite::Result<()> {
    for p in placed {
        let key = serde_json::to_string(&p.spot).expect("자리는 늘 직렬화된다");
        tx.execute(
            "INSERT OR REPLACE INTO spots (spot, repo, x, y) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![key, p.spot.repo().0, p.at.x, p.at.y],
        )?;
    }
    Ok(())
}
