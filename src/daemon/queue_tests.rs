//! 병합 큐를 실제 bash·git·flock으로 돌린다: 원격(bare), 원래 체크아웃, 가짜 check.sh·ship.sh.
//! Windows 대상 시험은 스크립트를 WSL로 띄우는 길(`wsl::command`)을 `wsl::tests`가 본다.

use super::*;

#[test]
fn scripts_use_the_exit_codes() {
    for (script, code) in [
        (STACK, CONFLICT),
        (STACK, CHECK_FAILED),
        (PUBLISH, MOVED),
        (ADVANCE, MOVED),
        (ADVANCE, DIRTY),
    ] {
        assert!(script.contains(&format!("exit {code}")), "{code}");
    }
}

#[cfg(unix)]
mod git_runs {
    use super::*;
    use crate::db;
    use crate::graph::{Kind, Launched, NewChild, NodeId, Place, Round, TreeLimits};
    use crate::protocol::{GateState, Judgment};
    use std::process::Command;
    use std::time::Instant;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn launched(token: &str) -> Launched {
        Launched {
            token: token.into(),
            server: None,
            account: None,
        }
    }

    /// 원격과 원래 체크아웃(main), 데몬 하나. check.sh는 `bad` 파일이 있으면 실패하고 도는 차례를 `check-runs`에 적는다.
    struct Repo {
        dir: tempfile::TempDir,
        main: PathBuf,
        app: Arc<App>,
        lead: NodeId,
        reviewer: NodeId,
        lock: String,
    }

    fn repo() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join("scripts/gate")).unwrap();
        git(
            dir.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.email", "t@t"]);
        git(&main, &["config", "user.name", "t"]);
        std::fs::write(
            main.join("scripts/gate/check.sh"),
            "echo \"$(git rev-parse HEAD)\" >>\"$(git rev-parse --path-format=absolute --git-common-dir)/check-runs\"\n[ ! -e bad ] && echo checked\n",
        )
        .unwrap();
        std::fs::write(
            main.join("scripts/gate/ship.sh"),
            "echo \"ship $1 $2\"; git rev-parse HEAD\n",
        )
        .unwrap();
        std::fs::write(main.join("f.txt"), "0\n").unwrap();
        git(&main, &["add", "-A"]);
        git(&main, &["commit", "-qm", "init"]);
        git(
            &main,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&main, &["push", "-q", "-u", "origin", "main"]);

        let db_path = dir.path().join("loom.db");
        let database = db::open(&db_path).unwrap();
        database
            .lock()
            .execute(
                "INSERT INTO repos (id, path) VALUES (1, ?1)",
                [main.to_string_lossy()],
            )
            .unwrap();
        let app = Arc::new(
            App::new(
                database,
                &db_path,
                "127.0.0.1:0".parse().unwrap(),
                None,
                None,
            )
            .unwrap(),
        );
        let place = Place {
            repo: RepoId(1),
            worktree: main.to_string_lossy().into_owned(),
        };
        let lead = app
            .graph
            .add_root(&place, None, None, &launched("lead"), |_| Ok(()))
            .unwrap();
        let reviewer = child(&app, lead, "reviewer", None);
        let lock = dir.path().join("check.lock").to_string_lossy().into_owned();
        Repo {
            dir,
            main,
            app,
            lead,
            reviewer,
            lock,
        }
    }

    fn child(app: &App, parent: NodeId, name: &str, role: Option<&str>) -> NodeId {
        app.graph
            .add_child(
                parent,
                NewChild {
                    name,
                    worktree: None,
                    agent: None,
                    brief: "",
                    role,
                    task: name,
                },
                TreeLimits {
                    max_depth: None,
                    max_children: None,
                },
                &launched(name),
                |_| Ok(()),
            )
            .unwrap()
    }

    impl Repo {
        /// `from`에서 브랜치를 만들어 파일 하나를 쓴 커밋.
        fn branch(&self, name: &str, from: &str, file: &str, text: &str) -> String {
            git(&self.main, &["checkout", "-q", "-b", name, from]);
            std::fs::write(self.main.join(file), text).unwrap();
            git(&self.main, &["add", "-A"]);
            git(&self.main, &["commit", "-qm", name]);
            let head = git(&self.main, &["rev-parse", "HEAD"]);
            git(&self.main, &["checkout", "-q", "main"]);
            head
        }

        /// 브랜치를 큐에 세운다. 담당은 총괄(스스로 관문), 판정은 총괄이 띄운 검토자다.
        fn queue(
            &self,
            owner: NodeId,
            gate: NodeId,
            reviewer: NodeId,
            branch: &str,
            head: &str,
            target: &str,
        ) {
            let round = Round {
                branch,
                target,
                worktree: "/wt",
                head,
            };
            self.app.graph.ready(owner, &round, gate, None).unwrap();
            assert_eq!(
                self.app
                    .graph
                    .verdict(reviewer, branch, Judgment::Pass, None, None),
                Ok(GateState::Passed)
            );
            self.mail(owner);
            self.mail(gate);
        }

        fn queue_main(&self, branch: &str, head: &str) {
            self.queue(self.lead, self.lead, self.reviewer, branch, head, "main");
        }

        async fn batch(&self, target: &str) {
            batch(&self.app, RepoId(1), target, &self.lock)
                .await
                .unwrap();
        }

        fn state(&self, branch: &str) -> GateState {
            let db = rusqlite::Connection::open(self.dir.path().join("loom.db")).unwrap();
            let state: String = db
                .query_row(
                    "SELECT state FROM gates WHERE branch = ?1",
                    [branch],
                    |row| row.get(0),
                )
                .unwrap();
            serde_json::from_str(&state).unwrap()
        }

        fn checks(&self) -> usize {
            std::fs::read_to_string(self.main.join(".git/check-runs"))
                .map_or(0, |t| t.lines().count())
        }

        fn mail(&self, node: NodeId) -> Vec<String> {
            let taken = self.app.graph.take(node, Kind::ALL, usize::MAX).unwrap();
            let ids: Vec<_> = taken.iter().map(|d| d.id).collect();
            self.app.graph.ack(node, &ids).unwrap();
            taken
                .iter()
                .map(|d| crate::profile::message_text(std::slice::from_ref(d)))
                .collect()
        }

        fn batches(&self) -> Vec<(u32, Vec<String>, BatchResult)> {
            let mut found: Vec<_> = self
                .app
                .graph
                .recent_events()
                .unwrap()
                .into_iter()
                .filter_map(|r| match r.entry {
                    Entry::Batch {
                        step,
                        members,
                        result,
                        ..
                    } => Some((
                        step,
                        members.into_iter().map(|m| m.branch).collect(),
                        result,
                    )),
                    _ => None,
                })
                .collect();
            found.reverse();
            found
        }
    }

    #[tokio::test]
    async fn a_batch_is_stacked_checked_once_published_and_shipped() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        let b = r.branch("b", "main", "b.txt", "b");
        r.queue_main("a", &a);
        r.queue_main("b", &b);
        r.batch("main").await;
        assert_eq!(r.checks(), 1);
        assert_eq!(
            (r.state("a"), r.state("b")),
            (GateState::Merged, GateState::Merged)
        );
        let tip = git(&r.main, &["rev-parse", "main"]);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), tip);
        assert_eq!(git(&r.main, &["rev-parse", "main^2"]), b);
        assert_eq!(git(&r.main, &["rev-parse", "main^1^2"]), a);
        let merged = r.mail(r.lead);
        assert_eq!(merged.len(), 2, "{merged:?}");
        assert!(
            merged[0].contains("kind: merged)\nbranch: a\n"),
            "{}",
            merged[0]
        );
        assert!(merged[1].contains("deploy: confirmed"), "{}", merged[1]);
        assert_eq!(
            r.batches(),
            [(0, vec!["a".to_owned(), "b".to_owned()], BatchResult::Passed)]
        );
        assert!(
            !r.main.join(".git/loom-batch").exists(),
            "the temp worktree is gone"
        );
    }

    #[tokio::test]
    async fn a_conflict_is_rework_and_the_rest_merges() {
        let r = repo();
        let a = r.branch("a", "main", "f.txt", "a\n");
        let b = r.branch("b", "main", "f.txt", "b\n");
        let c = r.branch("c", "main", "c.txt", "c");
        for (branch, head) in [("a", &a), ("b", &b), ("c", &c)] {
            r.queue_main(branch, head);
        }
        r.batch("main").await;
        assert_eq!(r.state("b"), GateState::Working);
        assert_eq!(
            (r.state("a"), r.state("c")),
            (GateState::Merged, GateState::Merged)
        );
        assert_eq!(r.checks(), 1);
        let to_owner = r.mail(r.lead);
        let rework = to_owner
            .iter()
            .find(|m| m.contains("kind: rework)\nbranch: b\n"))
            .unwrap();
        assert!(rework.contains("conflicts with main"), "{rework}");
        assert!(matches!(
            &r.batches()[0].2,
            BatchResult::Conflict { branch } if branch == "b"
        ));
    }

    /// 넷 가운데 하나가 검사를 깬다: 그 브랜치만 반려되고 나머지는 다음 묶음에서 병합된다. 검사는 1 + log2(4) = 3번이다.
    #[tokio::test]
    async fn bisect_finds_the_one_bad_branch_and_requeues_the_rest() {
        let r = repo();
        let mut heads = Vec::new();
        for name in ["a", "b", "c", "d"] {
            let file = if name == "b" { "bad" } else { name };
            heads.push((name, r.branch(name, "main", file, name)));
        }
        for (name, head) in &heads {
            r.queue_main(name, head);
        }
        r.batch("main").await;
        assert_eq!(r.checks(), 3);
        assert_eq!(r.state("b"), GateState::Working);
        for name in ["a", "c", "d"] {
            assert_eq!(r.state(name), GateState::Passed, "{name}");
        }
        assert_eq!(
            git(&r.main, &["rev-parse", "main"]),
            git(&r.main, &["rev-parse", "origin/main"])
        );
        let rework = r.mail(r.lead);
        let rework = rework
            .iter()
            .find(|m| m.contains("kind: rework)\nbranch: b\n"))
            .unwrap();
        assert!(
            rework.contains("\nlog: ") && rework.contains("bisect step"),
            "{rework}"
        );
        let steps: Vec<_> = r
            .batches()
            .into_iter()
            .map(|(s, m, res)| (s, m.join(""), res))
            .collect();
        assert_eq!(
            steps,
            [
                (0, "abcd".to_owned(), BatchResult::CheckFailed),
                (1, "ab".to_owned(), BatchResult::CheckFailed),
                (2, "a".to_owned(), BatchResult::Passed),
            ]
        );
        // 다음 묶음은 처음 큐 순서로 나머지를 병합한다.
        r.batch("main").await;
        assert_eq!(r.checks(), 4);
        for name in ["a", "c", "d"] {
            assert_eq!(r.state(name), GateState::Merged, "{name}");
        }
        assert_eq!(git(&r.main, &["rev-parse", "main^2"]), heads[3].1);
    }

    /// 데몬이 묶음 중에 끝났다. 검사 전이면 큐에 다시 서고, 검사를 지났으면 다시 띄운 데몬이 게시를 잇는다.
    #[tokio::test]
    async fn a_restarted_daemon_requeues_or_publishes_a_stale_batch() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.app.graph.batch_open(RepoId(1), "main", &["a"]).unwrap();
        // 끝난 데몬이 남긴 임시 워크트리.
        git(
            &r.main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                ".git/loom-batch",
                "main",
            ],
        );
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Passed);
        assert!(r.app.graph.stale_batches().unwrap().is_empty());
        assert!(!r.main.join(".git/loom-batch").exists());
        assert_eq!(r.checks(), 0);

        // 검사를 지난 묶음 끝을 남기고 끝났다.
        let base = git(&r.main, &["rev-parse", "main"]);
        let spare = r.dir.path().join("spare");
        git(
            &r.main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                spare.to_str().unwrap(),
                "main",
            ],
        );
        git(
            &spare,
            &["merge", "-q", "--no-ff", "-m", "Merge branch 'a'", &a],
        );
        let tip = git(&spare, &["rev-parse", "HEAD"]);
        git(
            &r.main,
            &["worktree", "remove", "--force", spare.to_str().unwrap()],
        );
        let id = r.app.graph.batch_open(RepoId(1), "main", &["a"]).unwrap();
        r.app.graph.batch_checked(id, &base, &tip).unwrap();
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Merged);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), tip);
        assert_eq!(r.checks(), 0, "no second check");
        assert!(
            r.batches()
                .iter()
                .any(|(_, _, res)| *res == BatchResult::Interrupted)
        );
    }

    /// 한 호스트에서 check.sh는 한 번에 하나다. check.sh가 남긴 자식은 잠금을 쥐지 않는다(옛 `loom merge` 검토 P3-e).
    #[tokio::test]
    async fn one_check_runs_at_a_time_and_a_leftover_child_keeps_no_lock() {
        let r = repo();
        std::fs::write(
            r.main.join("scripts/gate/check.sh"),
            "(setsid sleep 5 >/dev/null 2>&1 &) ; echo checked\n",
        )
        .unwrap();
        git(&r.main, &["commit", "-qam", "check leaves a child"]);
        git(&r.main, &["push", "-q", "origin", "main"]);
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        std::fs::File::create(&r.lock).unwrap();
        let mut holder = Command::new("flock")
            .args([r.lock.as_str(), "sleep", "2"])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        r.batch("main").await;
        holder.wait().unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(1500),
            "the batch waited for the other check"
        );
        assert_eq!(r.state("a"), GateState::Merged);
        let free = Command::new("flock")
            .args(["-n", r.lock.as_str(), "true"])
            .status()
            .unwrap();
        assert!(free.success(), "the leftover sleep keeps no lock");
    }

    /// 코디 브랜치가 대상이면 코디 워크트리를 빨리 감는다. 미커밋 변경이 있으면 반려다. 원격과 루트는 건드리지 않는다.
    #[tokio::test]
    async fn a_coordinator_target_fast_forwards_only_a_clean_worktree() {
        let r = repo();
        let coordinator = child(
            &r.app,
            r.lead,
            "코디",
            Some(crate::protocol::COORDINATOR_ROLE),
        );
        let worker = child(&r.app, coordinator, "worker", None);
        let reviewer = child(&r.app, coordinator, "coord-reviewer", None);
        let wt = r.dir.path().join("coord");
        git(
            &r.main,
            &["worktree", "add", "-q", "-b", "코디", wt.to_str().unwrap()],
        );
        let origin = git(&r.main, &["rev-parse", "origin/main"]);
        let a = r.branch("a", "코디", "a.txt", "a");
        r.queue(worker, coordinator, reviewer, "a", &a, "코디");
        r.batch("코디").await;
        assert_eq!(r.state("a"), GateState::Merged);
        assert_eq!(git(&wt, &["rev-parse", "HEAD^2"]), a);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), origin);
        assert!(r.mail(r.lead).is_empty(), "no root copy");
        assert_eq!(r.mail(worker).len(), 1);

        let coord_head = git(&wt, &["rev-parse", "HEAD"]);
        let b = r.branch("b", "코디", "b.txt", "b");
        r.queue(worker, coordinator, reviewer, "b", &b, "코디");
        std::fs::write(wt.join("wip.txt"), "x").unwrap();
        r.batch("코디").await;
        assert_eq!(r.state("b"), GateState::Working);
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), coord_head);
        assert_eq!(r.checks(), 1, "a dirty target is not checked");
        assert!(
            r.mail(worker)
                .iter()
                .any(|m| m.contains("uncommitted changes"))
        );
    }
}
