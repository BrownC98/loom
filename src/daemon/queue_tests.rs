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

#[test]
fn a_queued_worktree_is_read_through_the_repo_location() {
    let member = Queued {
        branch: "a".into(),
        owner: crate::graph::NodeId(1),
        head: "c0ffee".into(),
        worktree: "/home/a/wt".into(),
        since: 0,
        waited: 0,
    };
    let want = match cfg!(windows) {
        true => "//wsl.localhost/Ubuntu/home/a/wt",
        false => "/home/a/wt",
    };
    assert_eq!(
        source_of(Path::new(r"\\wsl.localhost\Ubuntu\home\a\repo"), &member),
        PathBuf::from(want)
    );
}

#[cfg(unix)]
mod git_runs {
    use super::*;
    use crate::db;
    use crate::graph::{Launched, NewChild, NodeId, Place, Round};
    use crate::protocol::{GateState, Judgment, Kind, TreeLimits};
    use std::process::Command;
    use std::time::Instant;

    static REVIEWERS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
            rules: None,
        }
    }

    struct Repo {
        dir: tempfile::TempDir,
        main: PathBuf,
        app: Arc<App>,
        lead: NodeId,
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
                tokio::sync::mpsc::unbounded_channel().0,
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
        let lock = dir.path().join("check.lock").to_string_lossy().into_owned();
        Repo {
            dir,
            main,
            app,
            lead,
            lock,
        }
    }

    fn child(
        app: &App,
        parent: NodeId,
        name: &str,
        role: Option<&str>,
        review: Option<&str>,
    ) -> NodeId {
        app.graph
            .add_child(
                parent,
                NewChild {
                    name,
                    worktree: None,
                    branched_from: None,
                    agent: None,
                    brief: "",
                    role,
                    task: name,
                    review,
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

    fn owner(
        app: &App,
        parent: NodeId,
        name: &str,
        role: &str,
        worktree: &Path,
        branched_from: Option<&str>,
    ) -> NodeId {
        app.graph
            .add_child(
                parent,
                NewChild {
                    name,
                    worktree: Some(worktree.to_str().unwrap()),
                    branched_from,
                    agent: None,
                    brief: "",
                    role: Some(role),
                    task: name,
                    review: None,
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

    async fn readied(app: &Arc<App>, token: &str, branch: &str, cwd: &Path) -> (String, String) {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let req = crate::daemon::gate::ReadyRequest {
            branch: branch.into(),
            cwd: cwd.to_string_lossy().into_owned(),
        };
        let readied = crate::daemon::gate::agent_ready(
            axum::extract::State(app.clone()),
            headers,
            axum::Json(req),
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"))
        .0;
        (readied.gate, readied.target)
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", file]);
    }

    #[tokio::test]
    async fn the_gate_follows_the_branch_a_worktree_was_made_from_not_the_role_name() {
        let r = repo();
        let upper = r.dir.path().join("upper");
        git(
            &r.main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "조율",
                upper.to_str().unwrap(),
            ],
        );
        let coordinator = owner(&r.app, r.lead, "c1", "조율자", &upper, Some("main"));
        let lower = r.dir.path().join("lower");
        git(
            &upper,
            &["worktree", "add", "-q", "-b", "a", lower.to_str().unwrap()],
        );
        owner(&r.app, coordinator, "w", "아무 역할", &lower, Some("조율"));
        commit(&lower, "a.txt");
        commit(&upper, "c.txt");
        git(&upper, &["checkout", "-q", "-b", "딴길"]);
        assert_eq!(
            readied(&r.app, "w", "a", &lower).await,
            ("c1".into(), "조율".into())
        );
        assert_eq!(
            r.app
                .graph
                .verdict(coordinator, "a", Judgment::Pass, None, None),
            Ok(GateState::Passed)
        );
        git(&upper, &["checkout", "-q", "조율"]);
        assert_eq!(
            readied(&r.app, "c1", "조율", &upper).await,
            ("c1".into(), "main".into())
        );
    }

    impl Repo {
        fn branch(&self, name: &str, from: &str, file: &str, text: &str) -> String {
            git(&self.main, &["checkout", "-q", "-b", name, from]);
            std::fs::write(self.main.join(file), text).unwrap();
            git(&self.main, &["add", "-A"]);
            git(&self.main, &["commit", "-qm", name]);
            let head = git(&self.main, &["rev-parse", "HEAD"]);
            git(&self.main, &["checkout", "-q", "main"]);
            head
        }

        fn removal(&self, name: &str, file: &str) -> String {
            git(&self.main, &["checkout", "-q", "-b", name, "main"]);
            git(&self.main, &["rm", "-q", file]);
            git(&self.main, &["commit", "-qm", name]);
            let head = git(&self.main, &["rev-parse", "HEAD"]);
            git(&self.main, &["checkout", "-q", "main"]);
            head
        }

        fn accept(&self, worker: NodeId, coordinator: NodeId, branch: &str, head: &str) {
            let round = Round {
                branch,
                target: "코디",
                worktree: "/wt",
                head,
            };
            self.app
                .graph
                .ready(worker, &round, coordinator, None)
                .unwrap();
            assert_eq!(
                self.app
                    .graph
                    .verdict(coordinator, branch, Judgment::Pass, None, None),
                Ok(GateState::Passed)
            );
            self.mail(worker);
            self.mail(coordinator);
        }

        fn queue_at(&self, owner: NodeId, gate: NodeId, round: &Round) {
            self.app.graph.ready(owner, round, gate, None).unwrap();
            let reviewer = child(
                &self.app,
                gate,
                &format!("reviewer-{}", REVIEWERS.fetch_add(1, Ordering::Relaxed)),
                None,
                Some(round.branch),
            );
            assert_eq!(
                self.app
                    .graph
                    .verdict(reviewer, round.branch, Judgment::Pass, None, None),
                Ok(GateState::Passed)
            );
            self.mail(owner);
            self.mail(gate);
        }

        fn queue_main(&self, branch: &str, head: &str) {
            let round = Round {
                branch,
                target: "main",
                worktree: "/wt",
                head,
            };
            self.queue_at(self.lead, self.lead, &round);
        }

        async fn batch(&self, target: &str) {
            self.batch_after(target, 0).await;
        }

        async fn batch_after(&self, target: &str, moves: u32) -> bool {
            batch(&self.app, RepoId(1), target, &self.lock, moves)
                .await
                .unwrap()
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

        fn batch_dirs(&self) -> usize {
            std::fs::read_dir(self.main.join(".git/loom-batches")).map_or(0, |d| d.count())
        }

        fn sql(&self, sql: &str) {
            rusqlite::Connection::open(self.dir.path().join("loom.db"))
                .unwrap()
                .execute_batch(sql)
                .unwrap();
        }

        fn merged_sha(&self, branch: &str) -> Option<String> {
            rusqlite::Connection::open(self.dir.path().join("loom.db"))
                .unwrap()
                .query_row(
                    "SELECT merged FROM gates WHERE branch = ?1",
                    [branch],
                    |row| row.get(0),
                )
                .unwrap()
        }

        fn commit_main(&self, file: &str, text: &str) {
            std::fs::write(self.main.join(file), text).unwrap();
            git(&self.main, &["add", "-A"]);
            git(&self.main, &["commit", "-qm", file]);
            git(&self.main, &["push", "-q", "origin", "main"]);
        }

        fn hook(&self, name: &str, text: &str) {
            use std::os::unix::fs::PermissionsExt;
            let path = self.main.join(".git/hooks").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn publish_logs(&self) -> String {
            std::fs::read_dir(self.main.join(".git/loom-logs"))
                .unwrap()
                .filter_map(|e| {
                    let path = e.unwrap().path();
                    path.to_string_lossy()
                        .ends_with("-publish.log")
                        .then(|| std::fs::read_to_string(path).unwrap())
                })
                .collect()
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

        fn worktree(&self, name: &str, branch: &str) -> PathBuf {
            let wt = self.dir.path().join(name);
            git(
                &self.main,
                &["worktree", "add", "-q", wt.to_str().unwrap(), branch],
            );
            wt
        }

        fn place(&self, node: NodeId, worktree: &Path) {
            rusqlite::Connection::open(self.dir.path().join("loom.db"))
                .unwrap()
                .execute(
                    "UPDATE nodes SET worktree = ?2 WHERE id = ?1",
                    rusqlite::params![node.0, worktree.to_string_lossy()],
                )
                .unwrap();
        }

        fn count(&self, sql: &str, value: &dyn rusqlite::ToSql) -> u32 {
            rusqlite::Connection::open(self.dir.path().join("loom.db"))
                .unwrap()
                .query_row(sql, [value], |row| row.get(0))
                .unwrap()
        }

        fn exists(&self, node: NodeId) -> bool {
            self.count("SELECT count(*) FROM nodes WHERE id = ?1", &node.0) == 1
        }

        fn reviewers(&self, branch: &str) -> u32 {
            self.count("SELECT count(*) FROM nodes WHERE review = ?1", &branch)
        }

        fn has_branch(&self, branch: &str) -> bool {
            Command::new("git")
                .args(["rev-parse", "--verify", "--quiet"])
                .arg(format!("refs/heads/{branch}"))
                .current_dir(&self.main)
                .output()
                .unwrap()
                .status
                .success()
        }

        async fn until(&self, what: &str, mut done: impl FnMut() -> bool) {
            for _ in 0..400 {
                if done() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            panic!("timed out: {what}");
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
        assert_eq!(r.batch_dirs(), 0, "the temp worktree is gone");
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
        r.batch("main").await;
        assert_eq!(r.checks(), 4);
        for name in ["a", "c", "d"] {
            assert_eq!(r.state(name), GateState::Merged, "{name}");
        }
        assert_eq!(git(&r.main, &["rev-parse", "main^2"]), heads[3].1);
    }

    #[tokio::test]
    async fn a_restarted_daemon_requeues_or_publishes_a_stale_batch() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.app.graph.batch_open(RepoId(1), "main", &["a"]).unwrap();
        git(
            &r.main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                ".git/loom-batches/batch-1-0",
                "main",
            ],
        );
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Passed);
        assert!(r.app.graph.stale_batches().unwrap().is_empty());
        assert_eq!(r.batch_dirs(), 0);
        assert_eq!(r.checks(), 0);

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

    #[tokio::test]
    async fn a_coordinator_target_fast_forwards_only_a_clean_worktree() {
        let r = repo();
        let coordinator = child(&r.app, r.lead, "코디", None, None);
        let worker = child(&r.app, coordinator, "worker", None, None);
        let wt = r.dir.path().join("coord");
        git(
            &r.main,
            &["worktree", "add", "-q", "-b", "코디", wt.to_str().unwrap()],
        );
        let origin = git(&r.main, &["rev-parse", "origin/main"]);
        let a = r.branch("a", "코디", "a.txt", "a");
        r.accept(worker, coordinator, "a", &a);
        r.batch("코디").await;
        assert_eq!(r.state("a"), GateState::Merged);
        assert_eq!(git(&wt, &["rev-parse", "HEAD^2"]), a);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), origin);
        assert!(r.mail(r.lead).is_empty(), "no root copy");
        assert_eq!(r.mail(worker).len(), 1);

        let coord_head = git(&wt, &["rev-parse", "HEAD"]);
        let next = child(&r.app, coordinator, "next", None, None);
        let b = r.branch("b", "코디", "b.txt", "b");
        r.accept(next, coordinator, "b", &b);
        std::fs::write(wt.join("wip.txt"), "x").unwrap();
        r.batch("코디").await;
        assert_eq!(r.state("b"), GateState::Working);
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), coord_head);
        assert_eq!(r.checks(), 1, "a dirty target is not checked");
        assert!(
            r.mail(next)
                .iter()
                .any(|m| m.contains("uncommitted changes"))
        );
    }

    #[tokio::test]
    async fn a_batch_script_removes_only_its_own_worktree() {
        let r = repo();
        let other = r.main.join(".git/loom-batches/batch-99-0");
        std::fs::create_dir_all(&other).unwrap();
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Merged);
        assert!(other.exists(), "another batch's worktree stays");
    }

    #[tokio::test]
    async fn a_failed_push_ends_the_batch_as_failed_without_a_loop() {
        let r = repo();
        let base = git(&r.main, &["rev-parse", "main"]);
        r.hook("pre-push", "#!/bin/sh\nexit 1\n");
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(r.checks(), 1);
        assert!(r.app.graph.queued_targets().unwrap().is_empty());
        assert_eq!(git(&r.main, &["rev-parse", "main"]), base);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), base);
        assert!(
            r.mail(r.lead)
                .iter()
                .any(|m| m.contains("kind: failed)\nbranch: a\n")),
            "the root hears it"
        );
        assert!(r.publish_logs().contains("git push origin main failed"));
    }

    #[tokio::test]
    async fn only_the_checked_commit_is_pushed() {
        let r = repo();
        let base = git(&r.main, &["rev-parse", "main"]);
        r.hook(
            "post-merge",
            "#!/bin/sh\n[ -d .git ] || exit 0\necho x >unchecked && git add unchecked && git commit -qm unchecked\n",
        );
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), base);
        assert!(r.publish_logs().contains("moved after the fast-forward"));
    }

    #[tokio::test]
    async fn a_missing_repository_ends_the_members_as_failed() {
        let r = repo();
        for name in ["a", "b"] {
            let head = r.branch(name, "main", &format!("{name}.txt"), name);
            r.queue_main(name, &head);
        }
        r.sql("UPDATE gates SET repo = 7");
        r.app.graph.batch_open(RepoId(7), "main", &["b"]).unwrap();
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("b"), GateState::Failed);
        batch(&r.app, RepoId(7), "main", &r.lock, 0).await.unwrap();
        assert_eq!(r.state("a"), GateState::Failed);
        assert!(r.app.graph.stale_batches().unwrap().is_empty());
        assert!(r.app.graph.queued_targets().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_old_round_left_by_the_migration_fails_with_a_notice() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.sql(r#"UPDATE gates SET state = '"merging"', batch = NULL"#);
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert!(
            r.mail(r.lead)
                .iter()
                .any(|m| m.contains("kind: failed)\nbranch: a\n") && m.contains(UPGRADE_CAUSE))
        );
        assert_eq!(r.checks(), 0);
    }

    #[tokio::test]
    async fn a_branch_inside_an_earlier_branch_ends_merged_by_that_merge() {
        let r = repo();
        let b1 = r.branch("b1", "main", "b1.txt", "1");
        let b2 = r.branch("b2", "b1", "b2.txt", "2");
        r.queue_main("b2", &b2);
        r.queue_main("b1", &b1);
        r.batch("main").await;
        assert_eq!(
            (r.state("b2"), r.state("b1")),
            (GateState::Merged, GateState::Merged)
        );
        let tip = git(&r.main, &["rev-parse", "main"]);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), tip);
        assert_eq!(r.merged_sha("b1").as_deref(), Some(tip.as_str()));
        assert_eq!(r.merged_sha("b2").as_deref(), Some(tip.as_str()));
        assert!(r.publish_logs().contains(&format!("ship b2 {b2}")));
    }

    #[tokio::test]
    async fn a_ship_error_after_the_push_still_ends_merged() {
        let r = repo();
        r.commit_main("scripts/gate/ship.sh", "echo \"ship $1 $2\"; exit 12\n");
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Merged);
        let merged = r.mail(r.lead);
        assert!(
            merged
                .iter()
                .any(|m| m.contains("deploy: unknown (scripts/gate/ship.sh exited 12")),
            "{merged:?}"
        );
    }

    #[tokio::test]
    async fn every_trial_of_a_batch_stacks_on_the_same_base() {
        let r = repo();
        r.commit_main(
            "scripts/gate/check.sh",
            "common=$(git rev-parse --path-format=absolute --git-common-dir)\necho \"$(git rev-parse HEAD)\" >>\"$common/check-runs\"\nif [ ! -e \"$common/moved\" ]; then touch \"$common/moved\"; git update-ref refs/heads/코디 HEAD; fi\n[ ! -e bad ] && echo checked\n",
        );
        let coordinator = child(&r.app, r.lead, "코디", None, None);
        let worker = child(&r.app, coordinator, "worker", None, None);
        let wt = r.dir.path().join("coord");
        git(
            &r.main,
            &["worktree", "add", "-q", "-b", "코디", wt.to_str().unwrap()],
        );
        let base = git(&wt, &["rev-parse", "HEAD"]);
        let a = r.branch("a", "코디", "a.txt", "a");
        let b = r.branch("b", "코디", "bad", "b");
        r.accept(worker, coordinator, "a", &a);
        r.accept(worker, coordinator, "b", &b);
        r.batch("코디").await;
        assert_eq!(
            (r.state("a"), r.state("b")),
            (GateState::Passed, GateState::Working)
        );
        let runs = std::fs::read_to_string(r.main.join(".git/check-runs")).unwrap();
        let runs: Vec<&str> = runs.lines().collect();
        assert_eq!(runs.len(), 2, "{runs:?}");
        for (run, merges) in runs.iter().zip([2, 1]) {
            assert_eq!(
                git(&r.main, &["rev-parse", &format!("{run}~{merges}")]),
                base
            );
        }
    }

    #[tokio::test]
    async fn a_failed_final_write_keeps_the_batch_for_recovery() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.sql(
            r#"CREATE TRIGGER no_merge BEFORE UPDATE OF state ON gates WHEN NEW.state = '"merged"'
               BEGIN SELECT RAISE(ABORT, 'injected'); END;"#,
        );
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Merging);
        assert_eq!(r.app.graph.stale_batches().unwrap().len(), 1);
        r.sql("DROP TRIGGER no_merge");
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Merged);
        assert!(r.app.graph.stale_batches().unwrap().is_empty());
        assert_eq!(r.checks(), 1, "no second check");
    }

    #[tokio::test]
    async fn a_local_only_commit_on_the_target_fails_the_batch_before_a_check() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        std::fs::write(r.main.join("local.txt"), "local\n").unwrap();
        git(&r.main, &["add", "-A"]);
        git(&r.main, &["commit", "-qm", "local"]);
        assert!(!r.batch_after("main", 0).await);
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(r.checks(), 0);
        assert!(r.app.graph.queued_targets().unwrap().is_empty());
        assert!(
            r.mail(r.lead)
                .iter()
                .any(|m| m.contains("main has commits that are not on origin/main")),
            "the root hears it"
        );
    }

    #[tokio::test]
    async fn a_target_that_keeps_moving_ends_the_batch_at_the_move_limit() {
        let r = repo();
        r.commit_main(
            "scripts/gate/check.sh",
            "echo run >>\"$(git rev-parse --path-format=absolute --git-common-dir)/check-runs\"\ngit fetch -q origin main\nc=$(git commit-tree -m moved -p origin/main 'origin/main^{tree}')\ngit push -q origin \"$c:refs/heads/main\"\n",
        );
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        assert!(r.batch_after("main", 0).await);
        assert_eq!(r.state("a"), GateState::Passed);
        assert!(
            !r.batch_after("main", MOVE_LIMIT - 1).await,
            "the counter starts again after the members fail"
        );
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(r.checks(), 2);
        assert!(r.app.graph.queued_targets().unwrap().is_empty());
        assert!(
            r.mail(r.lead)
                .iter()
                .any(|m| m.contains("main moved during 3 batches in a row")),
            "the root hears it"
        );
    }

    #[tokio::test]
    async fn a_failed_push_keeps_a_commit_made_during_the_push() {
        let r = repo();
        let base = git(&r.main, &["rev-parse", "main"]);
        r.hook(
            "pre-push",
            "#!/bin/sh\necho x >during && git add during && git commit -qm during\nexit 1\n",
        );
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(git(&r.main, &["show", "main:during"]), "x");
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), base);
        assert!(r.publish_logs().contains("main moved during git push"));
    }

    #[tokio::test]
    async fn a_failed_push_keeps_an_untracked_file_at_a_path_the_batch_deleted() {
        let r = repo();
        r.commit_main("removed.txt", "base content\n");
        git(&r.main, &["config", "status.showUntrackedFiles", "no"]);
        let base = git(&r.main, &["rev-parse", "main"]);
        r.hook(
            "pre-push",
            "#!/bin/sh\nprintf 'user content\\n' >removed.txt\nexit 1\n",
        );
        let a = r.removal("a", "removed.txt");
        r.queue_main("a", &a);
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(
            std::fs::read_to_string(r.main.join("removed.txt")).unwrap(),
            "user content\n"
        );
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), base);
        assert!(
            r.publish_logs().contains(
                "git push origin main failed and the main checkout has uncommitted changes"
            )
        );
    }

    #[tokio::test]
    async fn a_branch_that_moves_during_the_check_is_rework_and_the_rest_requeues() {
        let r = repo();
        r.commit_main(
            "scripts/gate/check.sh",
            "git update-ref refs/heads/a HEAD\n",
        );
        let base = git(&r.main, &["rev-parse", "main"]);
        let a = r.branch("a", "main", "a.txt", "a");
        let b = r.branch("b", "main", "b.txt", "b");
        r.queue_main("a", &a);
        r.queue_main("b", &b);
        r.batch("main").await;
        assert_eq!(
            (r.state("a"), r.state("b")),
            (GateState::Working, GateState::Passed)
        );
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), base);
        assert_eq!(git(&r.main, &["rev-parse", "main"]), base);
    }

    #[tokio::test]
    async fn a_resumed_batch_checks_the_heads_again_until_origin_has_the_tip() {
        let r = repo();
        let base = git(&r.main, &["rev-parse", "main"]);
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
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
        git(&r.main, &["update-ref", "refs/heads/a", &tip]);
        git(
            &r.main,
            &["worktree", "remove", "--force", spare.to_str().unwrap()],
        );
        git(&r.main, &["merge", "-q", "--ff-only", &tip]);
        let id = r.app.graph.batch_open(RepoId(1), "main", &["a"]).unwrap();
        r.app.graph.batch_checked(id, &base, &tip).unwrap();
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Working);
        assert_eq!(git(&r.main, &["rev-parse", "origin/main"]), base);
        assert_eq!(git(&r.main, &["rev-parse", "main"]), base);
        let b = r.branch("b", "main", "b.txt", "b");
        r.queue_main("b", &b);
        r.batch("main").await;
        assert_eq!(r.state("b"), GateState::Merged);
    }

    #[tokio::test]
    async fn a_resumed_batch_on_a_target_past_its_tip_asks_for_repair() {
        let r = repo();
        let base = git(&r.main, &["rev-parse", "main"]);
        let a = r.branch("a", "main", "a.txt", "a");
        r.queue_main("a", &a);
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
        git(&r.main, &["merge", "-q", "--ff-only", &tip]);
        std::fs::write(r.main.join("later.txt"), "later\n").unwrap();
        git(&r.main, &["add", "-A"]);
        git(&r.main, &["commit", "-qm", "later"]);
        let later = git(&r.main, &["rev-parse", "main"]);
        let id = r.app.graph.batch_open(RepoId(1), "main", &["a"]).unwrap();
        r.app.graph.batch_checked(id, &base, &tip).unwrap();
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(git(&r.main, &["rev-parse", "main"]), later);
        assert!(
            r.mail(r.lead)
                .iter()
                .any(|m| m.contains("main needs repair: main moved past the batch tip")),
            "the root hears it"
        );
    }

    #[tokio::test]
    async fn a_resumed_batch_keeps_an_untracked_file_at_a_path_the_batch_deleted() {
        let r = repo();
        r.commit_main("removed.txt", "base content\n");
        git(&r.main, &["config", "status.showUntrackedFiles", "no"]);
        let base = git(&r.main, &["rev-parse", "main"]);
        let a = r.removal("a", "removed.txt");
        r.queue_main("a", &a);
        git(&r.main, &["merge", "-q", "--ff-only", &a]);
        std::fs::write(r.main.join("removed.txt"), "user content\n").unwrap();
        let id = r.app.graph.batch_open(RepoId(1), "main", &["a"]).unwrap();
        r.app.graph.batch_checked(id, &base, &a).unwrap();
        recover(&r.app, &r.lock).await;
        assert_eq!(r.state("a"), GateState::Failed);
        assert_eq!(git(&r.main, &["rev-parse", "main"]), a);
        assert_eq!(
            std::fs::read_to_string(r.main.join("removed.txt")).unwrap(),
            "user content\n"
        );
        assert!(
            r.mail(r.lead).iter().any(|m| m
                .contains("main needs repair: the main checkout has uncommitted changes")),
            "the root hears it"
        );
    }

    #[tokio::test]
    async fn a_dirty_owner_worktree_is_rework_and_a_removed_one_is_not() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        let b = r.branch("b", "main", "b.txt", "b");
        let wt = r.dir.path().join("wt-a");
        git(
            &r.main,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "a"],
        );
        let gone = r.dir.path().join("gone");
        for (branch, head, worktree) in [("a", &a, &wt), ("b", &b, &gone)] {
            let round = Round {
                branch,
                target: "main",
                worktree: worktree.to_str().unwrap(),
                head,
            };
            r.queue_at(r.lead, r.lead, &round);
        }
        std::fs::write(wt.join("a.txt"), "edited\n").unwrap();
        r.batch("main").await;
        assert_eq!(
            (r.state("a"), r.state("b")),
            (GateState::Working, GateState::Merged)
        );
        assert!(
            r.mail(r.lead)
                .iter()
                .any(|m| m.contains("is not clean at the reviewed commit")),
        );
    }

    fn round<'a>(branch: &'a str, target: &'a str, worktree: &'a Path, head: &'a str) -> Round<'a> {
        Round {
            branch,
            target,
            worktree: worktree.to_str().unwrap(),
            head,
        }
    }

    #[tokio::test]
    async fn a_merged_branch_loses_its_reviewers_owner_worktree_and_branch() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let a = r.branch("a", "main", "a.txt", "a");
        git(&r.main, &["push", "-q", "-u", "origin", "a"]);
        let wt = r.worktree("wt-a", "a");
        r.place(owner, &wt);
        r.queue_at(owner, owner, &round("a", "main", &wt, &a));
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Merged);
        r.until("branch a is gone", || !r.has_branch("a")).await;
        assert_eq!(r.reviewers("a"), 0);
        assert!(!r.exists(owner));
        assert!(r.exists(r.lead));
        assert!(!wt.exists());
        assert_eq!(git(&r.main, &["ls-remote", "origin", "refs/heads/a"]), "");
    }

    #[tokio::test]
    async fn a_dirty_worktree_keeps_its_branch_and_owner_and_tells_the_parent() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let a = r.branch("a", "main", "a.txt", "a");
        let wt = r.worktree("wt-a", "a");
        r.place(owner, &wt);
        r.queue_at(owner, owner, &round("a", "main", &wt, &a));
        std::fs::write(wt.join("notes.txt"), "wip\n").unwrap();
        r.batch("main").await;
        assert_eq!(r.state("a"), GateState::Merged);
        let mut seen = Vec::new();
        r.until("the parent hears of the kept worktree", || {
            seen.extend(r.mail(r.lead));
            seen.iter().any(|m| m.contains("has uncommitted changes"))
        })
        .await;
        let kept = seen
            .iter()
            .find(|m| m.contains("has uncommitted changes"))
            .unwrap();
        assert!(kept.contains("from: loom"), "{kept}");
        assert!(kept.contains(wt.to_str().unwrap()), "{kept}");
        assert_eq!(r.reviewers("a"), 0);
        assert!(r.exists(owner));
        assert!(r.has_branch("a"));
        assert!(wt.join("notes.txt").is_file());
    }

    #[tokio::test]
    async fn a_lead_owner_is_never_removed() {
        let r = repo();
        let a = r.branch("a", "main", "a.txt", "a");
        let wt = r.worktree("wt-a", "a");
        r.queue_at(r.lead, r.lead, &round("a", "main", &wt, &a));
        r.batch("main").await;
        r.until("branch a is gone", || !r.has_branch("a")).await;
        assert!(r.exists(r.lead));
        assert_eq!(r.reviewers("a"), 0);
        assert!(!wt.exists());
    }

    #[tokio::test]
    async fn an_owner_with_children_is_not_removed() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let helper = child(&r.app, owner, "helper", None, None);
        let a = r.branch("a", "main", "a.txt", "a");
        let wt = r.worktree("wt-a", "a");
        r.queue_at(owner, owner, &round("a", "main", &wt, &a));
        r.batch("main").await;
        r.until("branch a is gone", || !r.has_branch("a")).await;
        assert!(r.exists(owner));
        assert_eq!(
            r.app.graph.relations(helper).unwrap().parent.map(|p| p.id),
            Some(owner)
        );
        assert_eq!(r.reviewers("a"), 0);
    }

    fn coordinator_with_worker(r: &Repo) -> (NodeId, NodeId, PathBuf) {
        let coordinator = child(&r.app, r.lead, "코디", None, None);
        let worker = child(&r.app, coordinator, "worker", None, None);
        let wt = r.dir.path().join("coord");
        git(
            &r.main,
            &["worktree", "add", "-q", "-b", "코디", wt.to_str().unwrap()],
        );
        r.place(coordinator, &wt);
        r.place(worker, &wt);
        (coordinator, worker, wt)
    }

    #[tokio::test]
    async fn a_worker_branch_merged_into_a_coordinator_is_reclaimed() {
        let r = repo();
        let (coordinator, worker, coord) = coordinator_with_worker(&r);
        let a = r.branch("a", "코디", "a.txt", "a");
        let wt = r.worktree("wt-a", "a");
        r.app
            .graph
            .ready(worker, &round("a", "코디", &wt, &a), coordinator, None)
            .unwrap();
        r.app
            .graph
            .verdict(coordinator, "a", Judgment::Pass, None, None)
            .unwrap();
        r.batch("코디").await;
        assert_eq!(r.state("a"), GateState::Merged);
        r.until("branch a is gone", || !r.has_branch("a")).await;
        assert!(!r.exists(worker));
        assert!(!wt.exists());
        assert!(r.exists(coordinator));
        assert_eq!(git(&coord, &["rev-parse", "HEAD^2"]), a);
    }

    async fn reclaim_merged(r: &Repo, branch: &str) {
        r.sql(r#"UPDATE gates SET state = '"merged"'"#);
        crate::daemon::reclaim::run(r.app.clone(), RepoId(1), vec![branch.into()]).await;
    }

    #[tokio::test]
    async fn a_worktree_that_another_node_shares_stays() {
        let r = repo();
        let (coordinator, worker, coord) = coordinator_with_worker(&r);
        let a = r.branch("a", "코디", "a.txt", "a");
        git(&coord, &["merge", "-q", "--no-ff", "-m", "Merge a", "a"]);
        r.app
            .graph
            .ready(worker, &round("a", "코디", &coord, &a), coordinator, None)
            .unwrap();
        reclaim_merged(&r, "a").await;
        assert!(!r.exists(worker));
        assert!(r.exists(coordinator));
        assert!(coord.is_dir());
        assert!(!r.has_branch("a"));
    }

    #[tokio::test]
    async fn a_git_failure_keeps_the_owner_of_a_dirty_worktree() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let a = r.branch("a", "main", "a.txt", "a");
        let wt = r.worktree("wt-a", "a");
        r.place(owner, &wt);
        r.app
            .graph
            .ready(owner, &round("a", "main", &wt, &a), r.lead, None)
            .unwrap();
        std::fs::write(wt.join("notes.txt"), "wip\n").unwrap();
        std::fs::rename(r.main.join(".git"), r.dir.path().join("moved.git")).unwrap();
        reclaim_merged(&r, "a").await;
        assert!(r.exists(owner));
        assert!(wt.join("notes.txt").is_file());
    }

    #[tokio::test]
    async fn the_original_checkout_stays() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let elsewhere = r.dir.path().join("elsewhere");
        r.place(r.lead, &elsewhere);
        r.place(owner, &elsewhere);
        let b = r.branch("b", "main", "b.txt", "b");
        git(&r.main, &["merge", "-q", "--no-ff", "-m", "Merge b", "b"]);
        r.queue_at(owner, owner, &round("b", "main", &r.main, &b));
        std::fs::write(r.main.join("untracked.txt"), "x").unwrap();
        reclaim_merged(&r, "b").await;
        assert!(!r.has_branch("b"));
        assert!(!r.exists(owner));
        assert!(r.main.join("untracked.txt").is_file());
        assert_eq!(git(&r.main, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    }

    #[tokio::test]
    async fn a_reworked_branch_keeps_its_reviewers_and_branch() {
        let r = repo();
        let a = r.branch("a", "main", "f.txt", "a\n");
        let b = r.branch("b", "main", "f.txt", "b\n");
        r.queue_main("a", &a);
        r.queue_main("b", &b);
        r.batch("main").await;
        assert_eq!(
            (r.state("a"), r.state("b")),
            (GateState::Merged, GateState::Working)
        );
        r.until("branch a is gone", || !r.has_branch("a")).await;
        assert_eq!(r.reviewers("a"), 0);
        assert_eq!(r.reviewers("b"), 1);
        assert!(r.has_branch("b"));
    }

    #[tokio::test]
    async fn a_remote_branch_with_newer_commits_stays_after_the_merge() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let a = r.branch("a", "main", "a.txt", "a");
        git(&r.main, &["push", "-q", "-u", "origin", "a"]);
        let other = r.dir.path().join("other");
        git(
            r.dir.path(),
            &[
                "clone",
                "-q",
                "--branch",
                "a",
                r.dir.path().join("origin.git").to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["config", "user.email", "t@t"]);
        git(&other, &["config", "user.name", "t"]);
        std::fs::write(other.join("new.txt"), "new remote work\n").unwrap();
        git(&other, &["add", "-A"]);
        git(&other, &["commit", "-qm", "new remote work"]);
        git(&other, &["push", "-q", "origin", "a"]);
        let latest = git(&other, &["rev-parse", "HEAD"]);
        assert_ne!(latest, a);
        assert_eq!(git(&r.main, &["rev-parse", "origin/a"]), a);
        let wt = r.worktree("wt-a", "a");
        r.place(owner, &wt);
        r.queue_at(owner, owner, &round("a", "main", &wt, &a));
        r.batch("main").await;
        r.until("cleanup finishes", || !r.has_branch("a")).await;
        assert_eq!(
            git(&r.main, &["ls-remote", "origin", "refs/heads/a"])
                .split_whitespace()
                .next(),
            Some(latest.as_str()),
            "cleanup must preserve the remote branch containing unmerged work"
        );
    }

    #[tokio::test]
    async fn a_worktree_now_on_another_branch_stays_with_its_owner() {
        let r = repo();
        let owner = child(&r.app, r.lead, "owner", None, None);
        let a = r.branch("a", "main", "a.txt", "a");
        let wt = r.worktree("wt-a", "a");
        r.place(owner, &wt);
        r.app
            .graph
            .ready(owner, &round("a", "main", &wt, &a), owner, None)
            .unwrap();
        git(&r.main, &["merge", "-q", "--no-ff", "-m", "Merge a", "a"]);
        git(&wt, &["checkout", "-q", "-b", "new-task"]);
        std::fs::write(wt.join("new.txt"), "new work\n").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-qm", "new task"]);
        reclaim_merged(&r, "a").await;
        assert!(
            wt.is_dir(),
            "cleanup must preserve a worktree now used for another branch"
        );
        assert!(
            r.exists(owner),
            "cleanup must preserve the owner of the new task"
        );
    }
}
