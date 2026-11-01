pub use api::canvas::{Arranged, CardSize, Layout, Placed, Point, Spot};

use crate::db::Database;
use crate::graph::NodeId;
use crate::worktree::RepoId;

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
        for key in write(&tx, placed)? {
            tx.execute(
                &format!(
                    "DELETE FROM arranged_repos WHERE ?1 ->> 'kind' != 'repo' AND repo = {}",
                    repo_of("?1")
                ),
                [key],
            )?;
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
        tx.execute(
            &format!("DELETE FROM spots WHERE {} = ?1", repo_of("spot")),
            [repo.0],
        )?;
        tx.execute("DELETE FROM arranged_repos WHERE repo = ?1", [repo.0])?;
        tx.commit()
    }
}

fn repo_of(spot: &str) -> String {
    format!("coalesce({spot} ->> 'repo', (SELECT repo FROM nodes WHERE id = {spot} ->> 'node'))")
}

fn write(tx: &rusqlite::Transaction, placed: &[Placed]) -> rusqlite::Result<Vec<String>> {
    let mut keys = Vec::new();
    for p in placed {
        let key = serde_json::to_string(&p.spot).expect("자리는 늘 직렬화된다");
        tx.execute(
            "INSERT OR REPLACE INTO spots (spot, x, y) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, p.at.x, p.at.y],
        )?;
        keys.push(key);
    }
    Ok(keys)
}
