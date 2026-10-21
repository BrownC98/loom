//! 그래프에서 사람이 끌어 놓은 자리(저장소 상자, 워크트리 묶음, 카드). [정렬]이 저장소마다 지운다.
//! 근거: docs/design.md 「저장소와 워크트리」 배치, 승인 목업 docs/mockups/repo-drag.html v3.

use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::graph::NodeId;
use crate::worktree::RepoId;

/// 자리를 가진 상자. 자리의 기준은 상자마다 다르다.
/// 저장소 상자는 그래프 좌표, 묶음은 저장소 상자 안 좌표, 카드는 묶음 안 좌표다. 그래서 상자를 옮겨도 안의 자리는 그대로다.
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

pub struct Spots {
    db: Mutex<Connection>,
}

impl Spots {
    pub fn new(conn: Connection) -> Self {
        Self {
            db: Mutex::new(conn),
        }
    }

    /// 저장한 자리 모두.
    pub fn list(&self) -> rusqlite::Result<Vec<Placed>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT spot, x, y FROM spots")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (spot, x, y) = row?;
            // 형식이 옛것인 줄은 버린다. 그 상자는 자동 배치 자리에 선다.
            if let Ok(spot) = serde_json::from_str(&spot) {
                out.push(Placed {
                    spot,
                    at: Point { x, y },
                });
            }
        }
        Ok(out)
    }

    /// 놓은 상자와 그 상자가 민 상자들의 자리를 한 트랜잭션에 쓴다.
    pub fn put(&self, placed: &[Placed]) -> rusqlite::Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        for p in placed {
            let key = serde_json::to_string(&p.spot).expect("자리는 늘 직렬화된다");
            tx.execute(
                "INSERT OR REPLACE INTO spots (spot, repo, x, y) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![key, p.spot.repo().0, p.at.x, p.at.y],
            )?;
        }
        tx.commit()
    }

    /// 워크트리를 지웠다. 같은 경로에 새 워크트리를 만들면 옛 자리에 서지 않게 한다.
    pub fn forget_worktree(&self, repo: RepoId, path: &str) -> rusqlite::Result<()> {
        let key = serde_json::to_string(&Spot::Worktree {
            repo,
            path: path.into(),
        })
        .expect("자리는 늘 직렬화된다");
        self.db
            .lock()
            .unwrap()
            .execute("DELETE FROM spots WHERE spot = ?1", [key])?;
        Ok(())
    }

    /// [정렬]: 그 저장소의 저장소 상자·묶음·카드 자리를 모두 지운다.
    pub fn clear(&self, repo: RepoId) -> rusqlite::Result<()> {
        self.db
            .lock()
            .unwrap()
            .execute("DELETE FROM spots WHERE repo = ?1", [repo.0])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spots() -> (tempfile::TempDir, Spots) {
        let tmp = tempfile::tempdir().unwrap();
        let spots = Spots::new(crate::db::open(&tmp.path().join("t.db")).unwrap());
        (tmp, spots)
    }

    fn at(spot: Spot, x: f64) -> Placed {
        Placed {
            spot,
            at: Point { x, y: 1.0 },
        }
    }

    #[test]
    fn put_replaces_the_same_spot_and_clear_takes_one_repo() {
        let (_tmp, spots) = spots();
        let (a, b) = (RepoId(1), RepoId(2));
        let worktree = Spot::Worktree {
            repo: a,
            path: "/w".into(),
        };
        spots.put(&[at(worktree.clone(), 1.0)]).unwrap();
        spots.put(&[at(worktree.clone(), 5.0)]).unwrap();
        spots
            .put(&[
                at(
                    Spot::Node {
                        repo: a,
                        node: NodeId(3),
                    },
                    2.0,
                ),
                at(Spot::Repo { repo: b }, 3.0),
            ])
            .unwrap();
        let mut got = spots.list().unwrap();
        got.sort_by(|p, q| p.at.x.total_cmp(&q.at.x));
        assert_eq!(
            got,
            [
                at(
                    Spot::Node {
                        repo: a,
                        node: NodeId(3)
                    },
                    2.0
                ),
                at(Spot::Repo { repo: b }, 3.0),
                at(worktree.clone(), 5.0),
            ]
        );
        spots.forget_worktree(a, "/w").unwrap();
        assert_eq!(spots.list().unwrap().len(), 2);
        spots.clear(a).unwrap();
        assert_eq!(spots.list().unwrap(), [at(Spot::Repo { repo: b }, 3.0)]);
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;

    #[test]
    fn removing_a_repo_or_node_removes_its_spots() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&tmp.path().join("t.db")).unwrap();
        conn.execute_batch(
            r#"INSERT INTO repos (id, path) VALUES (1, '/a'), (2, '/b');
            INSERT INTO nodes (id, name, agent, state, repo) VALUES (7, 'n', 'null', '{}', 2);"#,
        )
        .unwrap();
        let spots = Spots::new(conn);
        let put = |spot| {
            spots
                .put(&[Placed {
                    spot,
                    at: Point { x: 0.0, y: 0.0 },
                }])
                .unwrap()
        };
        put(Spot::Repo { repo: RepoId(1) });
        put(Spot::Repo { repo: RepoId(2) });
        put(Spot::Node {
            repo: RepoId(2),
            node: NodeId(7),
        });
        let db = |sql: &str| spots.db.lock().unwrap().execute(sql, []).unwrap();
        db("DELETE FROM repos WHERE id = 1");
        db("DELETE FROM nodes WHERE id = 7");
        assert_eq!(
            spots.list().unwrap(),
            [Placed {
                spot: Spot::Repo { repo: RepoId(2) },
                at: Point { x: 0.0, y: 0.0 }
            }]
        );
    }
}
