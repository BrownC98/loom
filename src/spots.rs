//! 그래프에서 사람이 끌어 놓은 자리(저장소 상자, 워크트리 묶음, 카드)와 바꾼 카드 크기. [정렬]이 저장소마다 자리를 지운다. 크기는 지킨다.
//! 정렬한 모양을 지키는 저장소도 둔다. 그 저장소는 카드·묶음이 생기거나 커지면 화면이 자동 배치를 다시 저장한다.
//! 근거: docs/design.md 「저장소와 워크트리」 배치, 승인 목업 docs/mockups/repo-drag.html v3, docs/mockups/card-resize.html v3.

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

    /// 저장소 안의 상자(묶음, 카드)면 그 저장소. 저장소 상자는 저장소 밖 자리다.
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
    /// 끌지 않고 커진 상자가 이 상자를 밀기 전의 자리. 민 상자가 줄거나 사라지면 화면이 이 자리 쪽으로 되돌린다. 사람이 옮기면 없다.
    pub home: Option<Point>,
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

    /// 저장한 자리 모두.
    fn list(&self) -> rusqlite::Result<Vec<Placed>> {
        let db = self.db.lock();
        let mut stmt = db.prepare("SELECT spot, x, y, home_x, home_y FROM spots")?;
        let rows = stmt.query_map([], |row| {
            let home = match (row.get(3)?, row.get(4)?) {
                (Some(x), Some(y)) => Some(Point { x, y }),
                _ => None,
            };
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, home))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (spot, x, y, home) = row?;
            // 형식이 옛것인 줄은 버린다. 그 상자는 자동 배치 자리에 선다.
            if let Ok(spot) = serde_json::from_str(&spot) {
                out.push(Placed {
                    spot,
                    at: Point { x, y },
                    home,
                });
            }
        }
        Ok(out)
    }

    /// 놓은 상자와 그 상자가 민 상자들의 자리(밀기 전 자리 포함)를 한 트랜잭션에 쓴다. 묶음·카드를 놓은 저장소는 정렬한 모양을 더 지키지 않는다.
    pub fn put(&self, placed: &[Placed]) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        write(&tx, placed)?;
        for repo in placed.iter().filter_map(|p| p.spot.inside()) {
            tx.execute("DELETE FROM arranged_repos WHERE repo = ?1", [repo.0])?;
        }
        tx.commit()
    }

    /// 자동 배치 자리를 쓰고 그 저장소들이 정렬한 모양을 지키게 한다.
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

    /// 워크트리를 지웠다. 같은 경로에 새 워크트리를 만들면 옛 자리에 서지 않게 한다.
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

    /// 저장한 카드 크기 모두.
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

    /// 그 카드들을 기본 크기로 되돌린다.
    pub fn clear_sizes(&self, nodes: &[NodeId]) -> rusqlite::Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        for node in nodes {
            tx.execute("DELETE FROM card_sizes WHERE node = ?1", [node.0])?;
        }
        tx.commit()
    }

    /// [정렬]: 그 저장소의 저장소 상자·묶음·카드 자리를 모두 지운다. 정렬한 자리는 `put_arranged`가 다시 쓴다.
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
            "INSERT OR REPLACE INTO spots (spot, repo, x, y, home_x, home_y) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                key,
                p.spot.repo().0,
                p.at.x,
                p.at.y,
                p.home.map(|h| h.x),
                p.home.map(|h| h.y)
            ],
        )?;
    }
    Ok(())
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
            home: None,
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

    #[test]
    fn home_is_written_with_the_spot_and_put_again_replaces_it() {
        let (_tmp, spots) = spots();
        let pushed = Placed {
            home: Some(Point { x: 0.0, y: 1.0 }),
            ..at(Spot::Repo { repo: RepoId(2) }, 9.0)
        };
        spots.put(std::slice::from_ref(&pushed)).unwrap();
        assert_eq!(spots.list().unwrap(), [pushed]);
        spots
            .put(&[at(Spot::Repo { repo: RepoId(2) }, 0.0)])
            .unwrap();
        assert_eq!(
            spots.list().unwrap(),
            [at(Spot::Repo { repo: RepoId(2) }, 0.0)]
        );
    }

    #[test]
    fn arranged_repo_stays_until_a_box_inside_it_is_placed() {
        let (_tmp, spots) = spots();
        let (a, b) = (RepoId(1), RepoId(2));
        let card = Spot::Node {
            repo: a,
            node: NodeId(3),
        };
        spots
            .put_arranged(&Arranged {
                repos: vec![a, b],
                placed: vec![at(card.clone(), 1.0)],
            })
            .unwrap();
        assert_eq!(spots.layout().unwrap().placed, [at(card.clone(), 1.0)]);
        // 저장소 상자를 옮겨도 안의 모양은 그대로다.
        spots.put(&[at(Spot::Repo { repo: a }, 2.0)]).unwrap();
        assert_eq!(spots.layout().unwrap().arranged, [a, b]);
        spots.put(&[at(card, 4.0)]).unwrap();
        assert_eq!(spots.layout().unwrap().arranged, [b]);
        spots.clear(b).unwrap();
        assert_eq!(spots.layout().unwrap().arranged, []);
    }

    #[test]
    fn sizes_survive_clear_and_reset_per_card() {
        let (_tmp, spots) = spots();
        let size = |node, width| CardSize {
            node: NodeId(node),
            width,
            height: 300.0,
        };
        spots.put_sizes(&[size(1, 700.0), size(2, 800.0)]).unwrap();
        spots.put_sizes(&[size(1, 900.0)]).unwrap();
        spots.clear(RepoId(1)).unwrap();
        let mut got = spots.sizes().unwrap();
        got.sort_by_key(|s| s.node.0);
        assert_eq!(got, [size(1, 900.0), size(2, 800.0)]);
        spots.clear_sizes(&[NodeId(2), NodeId(3)]).unwrap();
        assert_eq!(spots.sizes().unwrap(), [size(1, 900.0)]);
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;

    #[test]
    fn removing_a_repo_or_node_removes_its_spots_and_size() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::db::open(&tmp.path().join("t.db")).unwrap();
        db.lock()
            .execute_batch(
                r#"INSERT INTO repos (id, path) VALUES (1, '/a'), (2, '/b');
            INSERT INTO nodes (id, name, agent, state, repo) VALUES (7, 'n', 'null', '{}', 2);"#,
            )
            .unwrap();
        let spots = Spots::new(db);
        let put = |spot| {
            spots
                .put(&[Placed {
                    spot,
                    at: Point { x: 0.0, y: 0.0 },
                    home: None,
                }])
                .unwrap()
        };
        put(Spot::Repo { repo: RepoId(1) });
        put(Spot::Repo { repo: RepoId(2) });
        put(Spot::Node {
            repo: RepoId(2),
            node: NodeId(7),
        });
        spots
            .put_sizes(&[CardSize {
                node: NodeId(7),
                width: 1.0,
                height: 1.0,
            }])
            .unwrap();
        let db = |sql: &str| spots.db.lock().execute(sql, []).unwrap();
        db("DELETE FROM repos WHERE id = 1");
        db("DELETE FROM nodes WHERE id = 7");
        assert_eq!(spots.sizes().unwrap(), []);
        assert_eq!(
            spots.list().unwrap(),
            [Placed {
                spot: Spot::Repo { repo: RepoId(2) },
                at: Point { x: 0.0, y: 0.0 },
                home: None,
            }]
        );
    }

    /// 자식 워크트리 묶음 자리는 부모 묶음 안 좌표다. 다른 워크트리 부모를 가진 노드가 생기거나 부모가 바뀌면 바깥 상자가 바뀌므로 그 묶음 자리를 지운다.
    #[test]
    fn a_new_cross_worktree_parent_or_adoption_removes_the_worktree_spot() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::db::open(&tmp.path().join("t.db")).unwrap();
        db.lock().execute_batch(
            r#"INSERT INTO repos (id, path) VALUES (1, '/a');
            INSERT INTO nodes (id, name, agent, state, repo, worktree) VALUES (1, 'main', 'null', '{}', 1, '/a');"#,
        )
        .unwrap();
        let spots = Spots::new(db);
        let worktree = |path: &str| Spot::Worktree {
            repo: RepoId(1),
            path: path.into(),
        };
        let put = |spot| {
            spots
                .put(&[Placed {
                    spot,
                    at: Point { x: 5.0, y: 5.0 },
                    home: None,
                }])
                .unwrap()
        };
        let kept = || {
            spots
                .list()
                .unwrap()
                .into_iter()
                .map(|p| p.spot)
                .collect::<Vec<_>>()
        };
        let db = |sql: &str| spots.db.lock().execute(sql, []).unwrap();
        put(worktree("/a"));
        put(worktree("/w"));
        // 같은 워크트리 부모: 바깥 상자가 그대로다.
        db(
            "INSERT INTO nodes (id, name, agent, parent, state, repo, worktree) VALUES (2, 'w', 'null', 1, '{}', 1, '/a')",
        );
        assert_eq!(kept(), [worktree("/a"), worktree("/w")]);
        // 다른 워크트리 부모: /w 묶음이 main 카드의 묶음 안으로 간다.
        db(
            "INSERT INTO nodes (id, name, agent, parent, state, repo, worktree) VALUES (3, 'c', 'null', 1, '{}', 1, '/w')",
        );
        assert_eq!(kept(), [worktree("/a")]);
        // 부모 노드를 지우면 자식은 뿌리가 되고 그 묶음 자리를 지운다.
        put(worktree("/w"));
        db("UPDATE nodes SET parent = NULL WHERE parent = 1");
        assert_eq!(kept(), []);
    }
}
