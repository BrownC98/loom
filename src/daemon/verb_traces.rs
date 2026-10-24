use clap::{CommandFactory, Parser};
use tempfile::TempDir;

use super::*;
use crate::graph::{Entry, Round, TurnCheck};
use crate::profile::AgentId;
use crate::protocol::Judgment;

const UNTRACED: [(&str, &str); 15] = [
    ("serve", "runs the daemon"),
    ("start", "starts or joins the daemon"),
    ("open", "opens the screen"),
    ("stop", "stops the daemon"),
    ("carry", "moves an old daemon's data"),
    ("roles", "reads the settings"),
    ("status", "reads the graph"),
    (
        "wait",
        "reads the mailbox; the delivery is traced by ack (`Delivered`)",
    ),
    (
        "show",
        "checks a file on the caller side; --doc that uses the file is traced",
    ),
    ("hook", "hook transport that the agent CLI runs"),
    ("cursor-hook", "hook transport that the agent CLI runs"),
    ("cursor-rules", "hook transport that the agent CLI runs"),
    ("guard", "hook transport that the agent CLI runs"),
    ("pty-host", "the node process host"),
    ("help", "clap help"),
];

struct Fixture {
    dir: TempDir,
    app: Arc<App>,
    place: Place,
    lead: NodeId,
    worker: NodeId,
    reviewer: NodeId,
}

const LEAD: &str = "lead-token";
const WORKER: &str = "worker-token";
const REVIEWER: &str = "reviewer-token";

fn launched(token: &str) -> Launched {
    Launched {
        token: token.into(),
        server: None,
        account: None,
        rules: None,
    }
}

fn claude() -> Option<AgentId> {
    Some(AgentId("claude".into()))
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let db_path = dir.path().join("loom.db");
    let database = db::open(&db_path).unwrap();
    database
        .lock()
        .execute(
            "INSERT INTO repos (id, path) VALUES (1, ?1)",
            [repo.to_string_lossy()],
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
        worktree: repo.to_string_lossy().into_owned(),
    };
    let lead = app
        .graph
        .add_root(&place, claude(), None, &launched(LEAD), |_| Ok(()))
        .unwrap();
    let mut fixture = Fixture {
        dir,
        app,
        place,
        lead,
        worker: lead,
        reviewer: lead,
    };
    fixture.worker = fixture.child("worker", WORKER);
    fixture.reviewer = fixture.child_of(fixture.worker, "reviewer", REVIEWER);
    fixture
}

impl Fixture {
    fn child(&self, name: &str, token: &str) -> NodeId {
        self.child_of(self.lead, name, token)
    }

    fn child_of(&self, parent: NodeId, name: &str, token: &str) -> NodeId {
        let child = NewChild {
            name,
            worktree: None,
            agent: claude(),
            brief: "",
            role: None,
            task: "t",
            review: None,
        };
        let limits = TreeLimits {
            max_depth: None,
            max_children: None,
        };
        self.app
            .graph
            .add_child(parent, child, limits, &launched(token), |_| Ok(()))
            .unwrap()
    }

    fn state(&self) -> State<Arc<App>> {
        State(self.app.clone())
    }

    fn mark(&self) -> u32 {
        self.app
            .graph
            .recent_events()
            .unwrap()
            .first()
            .map_or(0, |r| r.id)
    }

    fn since(&self, mark: u32) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self
            .app
            .graph
            .recent_events()
            .unwrap()
            .into_iter()
            .filter(|r| r.id > mark)
            .map(|r| r.entry)
            .collect();
        entries.reverse();
        entries
    }

    fn refused<T>(&self, verb: &str, mark: u32, result: Result<Json<T>, AgentError>) -> Refusal {
        assert!(result.is_err(), "{verb}: the refusal scene was not refused");
        let tool = format!("loom {verb}");
        self.since(mark)
            .into_iter()
            .find_map(|e| match e {
                Entry::Refused {
                    via: Some(via),
                    reason,
                    ..
                } if via.tool == tool => Some(reason),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{verb}: the refusal left no event"))
    }

    fn wrote(&self, verb: &str, mark: u32, found: impl Fn(&Entry) -> bool) {
        let entries = self.since(mark);
        assert!(
            entries.iter().any(found),
            "{verb}: the success left no event: {entries:?}"
        );
    }
}

fn ok<T>(verb: &str, result: Result<Json<T>, AgentError>) {
    if let Err((status, text)) = result {
        panic!("{verb}: the success scene failed: {status} {text}");
    }
}

fn headers(token: &str, verb: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers.insert(gate::TOOL_HEADER, format!("loom {verb}").parse().unwrap());
    headers
}

fn message_from(node: NodeId) -> impl Fn(&Entry) -> bool {
    move |e| matches!(e, Entry::Message { from, .. } if from.id == Some(node))
}

#[tokio::test]
async fn every_agent_verb_leaves_an_event_on_success_and_on_refusal() {
    let t = fixture();
    let verbs: Vec<String> = crate::Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    for (name, _) in UNTRACED {
        assert!(
            name == "help" || verbs.iter().any(|v| v == name),
            "UNTRACED has {name}, but the CLI has no such command"
        );
    }
    for verb in verbs
        .iter()
        .filter(|v| UNTRACED.iter().all(|(n, _)| n != v))
    {
        scene(&t, verb).await;
    }
}

async fn scene(t: &Fixture, verb: &str) {
    let mark = t.mark();
    match verb {
        "spawn" => {
            let req = SpawnRequest {
                run: SpawnRun::Role {
                    name: "made".into(),
                    role: "조율".into(),
                },
                brief: "no heads".into(),
                worktree: None,
            };
            let refused = agent_spawn(t.state(), headers(LEAD, verb), Json(req)).await;
            assert!(matches!(
                t.refused(verb, mark, refused),
                Refusal::MissingHeads(_)
            ));
            let mark = t.mark();
            t.child("made", "made-token");
            t.wrote(
                verb,
                mark,
                |e| matches!(e, Entry::Created { node, .. } if node.name == "made"),
            );
        }
        "note" => {
            let note = |to: &str| NoteRequest {
                to: to.into(),
                mode: Kind::Queue,
                body: "hi".into(),
            };
            let refused = agent_note(t.state(), headers(WORKER, verb), Json(note("nobody"))).await;
            t.refused(verb, mark, refused);
            let mark = t.mark();
            ok(
                verb,
                agent_note(t.state(), headers(WORKER, verb), Json(note(PARENT))).await,
            );
            t.wrote(verb, mark, message_from(t.worker));
        }
        "answer" => {
            let answer = |message| AnswerRequest {
                message,
                body: "yes".into(),
            };
            let refused = agent_answer(
                t.state(),
                headers(LEAD, verb),
                Json(answer(MessageId(999_999))),
            )
            .await;
            t.refused(verb, mark, refused);
            let ask = AskRequest {
                question: "Which file?".into(),
                doc: None,
            };
            let Json(AskResult::Parent(sent)) =
                agent_ask(t.state(), headers(WORKER, "ask"), Json(ask))
                    .await
                    .unwrap()
            else {
                panic!("a child asks its parent");
            };
            let mark = t.mark();
            ok(
                verb,
                agent_answer(t.state(), headers(LEAD, verb), Json(answer(sent.id))).await,
            );
            t.wrote(verb, mark, message_from(t.lead));
        }
        "ask" => {
            let ask = |question: &str| AskRequest {
                question: question.into(),
                doc: None,
            };
            let refused = agent_ask(t.state(), headers(LEAD, verb), Json(ask(" "))).await;
            assert_eq!(t.refused(verb, mark, refused), Refusal::EmptyQuestion);
            let md = t.dir.path().join("r.md");
            std::fs::write(&md, "# r\n").unwrap();
            let link = format!("[r]({})", crate::user_doc::tests::file_url(&md));
            let mark = t.mark();
            let refused = agent_ask(
                t.state(),
                headers(LEAD, verb),
                Json(ask(&format!(
                    "{link} 승인할까요? (A: 예 / B: 아니오, 권장: A)"
                ))),
            )
            .await;
            assert!(matches!(
                t.refused(verb, mark, refused),
                Refusal::UncheckedDoc(_)
            ));
            let mark = t.mark();
            ok(
                verb,
                agent_ask(
                    t.state(),
                    headers(LEAD, verb),
                    Json(ask("배포할까요? (A: 예 / B: 아니오, 권장: A)")),
                )
                .await,
            );
            t.wrote(verb, mark, |e| matches!(e, Entry::Asked { .. }));
        }
        "done" => {
            let done = |summary: &str, images: Vec<PathBuf>| DoneRequest {
                summary: summary.into(),
                file: None,
                asked: Some("do it".into()),
                images,
                doc: None,
            };
            let image = vec![t.dir.path().join("none.png")];
            let refused =
                agent_done(t.state(), headers(WORKER, verb), Json(done("끝", image))).await;
            assert!(matches!(
                t.refused(verb, mark, refused),
                Refusal::NotImage(_)
            ));
            let mark = t.mark();
            ok(
                verb,
                agent_done(
                    t.state(),
                    headers(WORKER, verb),
                    Json(done("끝", Vec::new())),
                )
                .await,
            );
            t.wrote(verb, mark, message_from(t.worker));
        }
        "answered" => {
            let answered = |ids| Json(Some(AnsweredRequest { ids }));
            let refused = agent_answered(
                t.state(),
                headers(LEAD, verb),
                answered(vec![WaitId(999_999)]),
            )
            .await;
            t.refused(verb, mark, refused);
            let asked = t
                .app
                .graph
                .ask(t.lead, "남길까요? (A: 예 / B: 아니오, 권장: A)", None)
                .unwrap();
            let mark = t.mark();
            ok(
                verb,
                agent_answered(t.state(), headers(LEAD, verb), answered(vec![asked])).await,
            );
            t.wrote(
                verb,
                mark,
                |e| matches!(e, Entry::Unanswered { wait, .. } if *wait == asked),
            );
        }
        "adopt" => {
            let adopt = |name: &str| Json(AdoptRequest { name: name.into() });
            let refused = agent_adopt(t.state(), headers(LEAD, verb), adopt("nobody")).await;
            t.refused(verb, mark, refused);
            t.app
                .graph
                .add_root(&t.place, claude(), None, &launched("orphan-token"), |_| {
                    Ok(())
                })
                .unwrap();
            let orphan = t
                .app
                .graph
                .directory()
                .unwrap()
                .into_iter()
                .last()
                .unwrap()
                .name;
            let mark = t.mark();
            ok(
                verb,
                agent_adopt(t.state(), headers(LEAD, verb), adopt(&orphan)).await,
            );
            t.wrote(verb, mark, |e| matches!(e, Entry::Adopted { .. }));
        }
        "rm" => {
            let rm = |name: &str| Json(RemoveRequest { name: name.into() });
            let refused = agent_remove(t.state(), headers(LEAD, verb), rm("nobody")).await;
            t.refused(verb, mark, refused);
            t.child("spare", "spare-token");
            let mark = t.mark();
            ok(
                verb,
                agent_remove(t.state(), headers(LEAD, verb), rm("spare")).await,
            );
            t.wrote(
                verb,
                mark,
                |e| matches!(e, Entry::Removed { node } if node.name == "spare"),
            );
        }
        "relaunch" => {
            let relaunch = |name: &str| RelaunchRequest {
                name: Some(name.into()),
                to: None,
            };
            let refused =
                relaunch::agent_relaunch(t.state(), headers(WORKER, verb), Json(relaunch(PARENT)))
                    .await;
            assert!(matches!(
                t.refused(verb, mark, refused),
                Refusal::NotChild(_)
            ));
            let mark = t.mark();
            let _ =
                relaunch::agent_relaunch(t.state(), headers(LEAD, verb), Json(relaunch("worker")))
                    .await;
            t.wrote(
                verb,
                mark,
                |e| matches!(e, Entry::Relaunched { node, .. } if node.id == Some(t.worker)),
            );
        }
        "ready" => {
            let req = gate::ReadyRequest {
                branch: "b".into(),
                cwd: t.place.worktree.clone(),
            };
            let refused = gate::agent_ready(t.state(), headers(WORKER, verb), Json(req)).await;
            t.refused(verb, mark, refused);
            let round = Round {
                branch: "b",
                target: "main",
                worktree: &t.place.worktree,
                head: "c0ffee1",
            };
            let mark = t.mark();
            t.app.graph.ready(t.worker, &round, t.worker, None).unwrap();
            t.wrote(verb, mark, message_from(t.worker));
        }
        "verdict" => {
            let req = gate::VerdictRequest {
                branch: "nope".into(),
                judgment: Judgment::Pass,
                findings: None,
            };
            let refused = gate::agent_verdict(t.state(), headers(REVIEWER, verb), Json(req)).await;
            t.refused(verb, mark, refused);
            rusqlite::Connection::open(t.dir.path().join("loom.db"))
                .unwrap()
                .execute(
                    "UPDATE nodes SET review = 'b' WHERE id = ?1",
                    [t.reviewer.0],
                )
                .unwrap();
            let mark = t.mark();
            t.app
                .graph
                .verdict(t.reviewer, "b", Judgment::Pass, None, None)
                .unwrap();
            t.wrote(verb, mark, message_from(t.reviewer));
            let round = Round {
                branch: "c",
                target: "main",
                worktree: &t.place.worktree,
                head: "c0ffee2",
            };
            t.app.graph.ready(t.worker, &round, t.worker, None).unwrap();
            let mark = t.mark();
            let req = gate::VerdictRequest {
                branch: "c".into(),
                judgment: Judgment::Pass,
                findings: None,
            };
            let refused = gate::agent_verdict(t.state(), headers(REVIEWER, verb), Json(req)).await;
            assert!(matches!(
                t.refused(verb, mark, refused),
                Refusal::NotReviewer {
                    review: Some(_),
                    ..
                }
            ));
        }
        "fail" => {
            let fail = |branch: &str| gate::FailRequest {
                branch: branch.into(),
                cause: "broken".into(),
            };
            let refused =
                gate::agent_fail(t.state(), headers(WORKER, verb), Json(fail("nope"))).await;
            t.refused(verb, mark, refused);
            let mark = t.mark();
            t.app.graph.fail(t.worker, "b", "broken", None).unwrap();
            t.wrote(verb, mark, message_from(t.worker));
        }
        _ => panic!(
            "{verb}: no trace scene. Add a success and a refusal scene here, or add {verb} to UNTRACED with the reason"
        ),
    }
}

#[test]
fn a_nudged_turn_end_leaves_an_event() {
    let t = fixture();
    t.app.turn_ends.human(t.lead);
    t.app
        .turn_ends
        .said(t.lead, "A안과 B안 가운데 고르시면 진행합니다.".into());
    let mark = t.mark();
    assert!(t.app.turn_end(t.lead).is_some());
    t.wrote("turn end", mark, |e| {
        matches!(e, Entry::Nudged { node, check: TurnCheck::Unreported } if node.id == Some(t.lead))
    });
    let mark = t.mark();
    assert!(t.app.turn_end(t.lead).is_none());
    assert_eq!(t.since(mark), []);
}

impl Fixture {
    fn cursor(&self, name: &str) -> NodeId {
        let child = NewChild {
            name,
            worktree: None,
            agent: Some(AgentId("cursor".into())),
            brief: "",
            role: None,
            task: "t",
            review: None,
        };
        let limits = TreeLimits {
            max_depth: None,
            max_children: None,
        };
        self.app
            .graph
            .add_child(self.lead, child, limits, &launched(name), |_| Ok(()))
            .unwrap()
    }

    fn heard(&self, node: NodeId, turn: &str) {
        self.app.idle.open(node).finish(IdleEnd::Mail);
        self.prompt(node, turn);
    }

    fn prompt(&self, node: NodeId, turn: &str) {
        self.app
            .idle
            .turn_started(node, "beforeSubmitPrompt".into());
        self.app.idle.prompted(node, TurnId(turn.into()));
    }

    async fn said(&self, token: &str, turn: Option<&str>) {
        let text = SaidRequest {
            text: "끝".into(),
            turn: turn.map(|t| TurnId(t.into())),
        };
        ok(
            "said",
            agent_said(self.state(), headers(token, "said"), Json(text)).await,
        );
    }

    fn idle(
        &self,
        token: &str,
        turn: &str,
    ) -> impl Future<Output = Result<Json<Vec<Delivery>>, AgentError>> + Send + 'static {
        let req = IdleRequest {
            turn: Some(TurnId(turn.into())),
            ..IdleRequest::default()
        };
        agent_idle(self.state(), headers(token, "idle"), Json(Some(req)))
    }

    async fn idle_request(&self, token: &str, turn: &str) {
        let _ = tokio::time::timeout(Duration::from_millis(1), self.idle(token, turn)).await;
    }

    async fn held_idle_request(&self, token: &str, turn: &str) -> tokio::task::JoinHandle<()> {
        let idle = self.idle(token, turn);
        let held = tokio::spawn(async move {
            let _ = idle.await;
        });
        tokio::time::sleep(Duration::from_millis(1)).await;
        held
    }

    async fn tool_hook(&self, token: &str, turn: &str) {
        let req = TakeRequest {
            hook: "postToolUse".into(),
            kinds: vec![],
            limit_bytes: 8192,
            transcript: None,
            human_input: false,
            prompt: None,
            turn: Some(TurnId(turn.into())),
        };
        ok(
            "take",
            agent_take(self.state(), headers(token, "take"), Json(req)).await,
        );
    }

    async fn stop_hook(&self, token: &str, turn: &str) {
        let req = TurnEndRequest {
            turn: Some(TurnId(turn.into())),
        };
        let end = agent_turn_end(self.state(), headers(token, "turn-end"), Json(Some(req)));
        ok("turn-end", end.await);
    }

    fn relaunches(&self, mark: u32) -> Vec<Entry> {
        self.since(mark)
            .into_iter()
            .filter(|e| matches!(e, Entry::NoIdleWait { .. } | Entry::Relaunched { .. }))
            .collect()
    }
}

#[tokio::test(start_paused = true)]
async fn a_turn_end_without_an_idle_wait_relaunches_the_node_once() {
    let t = fixture();
    let deaf = t.cursor("deaf");
    let heard = t.cursor("heard");
    t.heard(deaf, "g1");
    t.heard(heard, "g1");

    let mark = t.mark();
    t.said("deaf", Some("g1")).await;
    t.said("heard", Some("g1")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE / 2).await;
    t.stop_hook("heard", "g1").await;
    let _wait = t.app.idle.open(heard);
    tokio::time::sleep(turn_end::IDLE_GRACE).await;
    let found = t.relaunches(mark);
    assert!(
        matches!(
            found.as_slice(),
            [
                Entry::NoIdleWait { node },
                Entry::Relaunched { node: again, error: Some(_), .. },
            ] if node.id == Some(deaf) && again.id == Some(deaf)
        ),
        "{found:?}"
    );

    let mark = t.mark();
    t.prompt(deaf, "g2");
    t.said("deaf", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    assert_eq!(t.relaunches(mark), []);
}

#[tokio::test(start_paused = true)]
async fn a_late_idle_request_answers_only_its_own_turn() {
    let t = fixture();
    let node = t.cursor("overtaken");
    t.heard(node, "g1");
    t.stop_hook("overtaken", "g1").await;
    t.prompt(node, "g2");
    t.idle_request("overtaken", "g1").await;
    let mark = t.mark();
    t.said("overtaken", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    let found = t.relaunches(mark);
    assert!(
        matches!(found.as_slice(), [Entry::NoIdleWait { node: n }, ..] if n.id == Some(node)),
        "{found:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_idle_request_keeps_the_watch_of_a_newer_turn() {
    let t = fixture();
    let node = t.cursor("overtaken");
    t.heard(node, "g1");
    t.stop_hook("overtaken", "g1").await;
    t.prompt(node, "g2");
    let mark = t.mark();
    t.said("overtaken", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE / 2).await;
    t.idle_request("overtaken", "g1").await;
    tokio::time::sleep(turn_end::IDLE_GRACE).await;
    let found = t.relaunches(mark);
    assert!(
        matches!(found.as_slice(), [Entry::NoIdleWait { node: n }, ..] if n.id == Some(node)),
        "{found:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_old_idle_wait_open_past_the_deadline_only_delays_the_relaunch() {
    let t = fixture();
    let node = t.cursor("overtaken");
    t.heard(node, "g1");
    t.stop_hook("overtaken", "g1").await;
    t.prompt(node, "g2");
    let mark = t.mark();
    t.said("overtaken", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE / 2).await;
    let old = t.held_idle_request("overtaken", "g1").await;
    tokio::time::sleep(turn_end::IDLE_GRACE).await;
    assert_eq!(t.relaunches(mark), [], "the old wait still listens");
    old.abort();
    let _ = old.await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    let found = t.relaunches(mark);
    assert!(
        matches!(found.as_slice(), [Entry::NoIdleWait { node: n }, ..] if n.id == Some(node)),
        "{found:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_tool_hook_keeps_the_watch_of_a_newer_turn() {
    let t = fixture();
    let node = t.cursor("overtaken");
    t.heard(node, "g1");
    t.stop_hook("overtaken", "g1").await;
    t.prompt(node, "g2");
    let mark = t.mark();
    t.said("overtaken", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE / 2).await;
    t.tool_hook("overtaken", "g1").await;
    tokio::time::sleep(turn_end::IDLE_GRACE).await;
    let found = t.relaunches(mark);
    assert!(
        matches!(found.as_slice(), [Entry::NoIdleWait { node: n }, ..] if n.id == Some(node)),
        "{found:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_turn_that_a_stop_hook_starts_is_watched() {
    let t = fixture();
    let woken = t.cursor("woken");
    let nudged = t.cursor("nudged");
    for node in [woken, nudged] {
        t.heard(node, "g1");
    }
    t.stop_hook("woken", "g1").await;
    t.idle_request("woken", "g1").await;
    t.tool_hook("woken", "g2").await;
    t.stop_hook("nudged", "g1").await;
    let mark = t.mark();
    t.said("woken", Some("g2")).await;
    t.said("nudged", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    let missed: Vec<_> = t
        .relaunches(mark)
        .into_iter()
        .filter_map(|e| match e {
            Entry::NoIdleWait { node } => node.id,
            _ => None,
        })
        .collect();
    assert_eq!(missed, [woken, nudged]);
}

#[tokio::test(start_paused = true)]
async fn an_unseen_old_turn_keeps_the_watch_of_a_newer_turn() {
    let t = fixture();
    let prompted = t.cursor("prompted");
    let followed = t.cursor("followed");
    for node in [prompted, followed] {
        t.heard(node, "g1");
    }
    t.stop_hook("prompted", "g1").await;
    t.prompt(prompted, "g3");
    t.stop_hook("followed", "g1").await;
    let mark = t.mark();
    t.said("prompted", Some("g3")).await;
    t.said("followed", Some("g3")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE / 2).await;
    for token in ["prompted", "followed"] {
        t.said(token, Some("g2")).await;
        t.stop_hook(token, "g2").await;
    }
    tokio::time::sleep(turn_end::IDLE_GRACE).await;
    let missed: Vec<_> = t
        .relaunches(mark)
        .into_iter()
        .filter_map(|e| match e {
            Entry::NoIdleWait { node } => node.id,
            _ => None,
        })
        .collect();
    assert_eq!(missed, [prompted, followed]);
}

#[tokio::test(start_paused = true)]
async fn a_draft_holds_every_turn_until_the_human_submits() {
    let t = fixture();
    let node = t.cursor("drafting");
    t.heard(node, "g1");
    t.stop_hook("drafting", "g1").await;
    human_typed(&t.app, node);
    t.tool_hook("drafting", "g2").await;
    let mark = t.mark();
    t.said("drafting", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    assert_eq!(
        t.relaunches(mark),
        [],
        "the draft is still in the input box"
    );

    t.stop_hook("drafting", "g2").await;
    t.prompt(node, "g3");
    t.said("drafting", Some("g3")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    let found = t.relaunches(mark);
    assert!(
        matches!(found.as_slice(), [Entry::NoIdleWait { node: n }, ..] if n.id == Some(node)),
        "{found:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_idle_request_keeps_the_typing_of_a_newer_turn() {
    let t = fixture();
    let node = t.cursor("drafting");
    t.heard(node, "g1");
    t.stop_hook("drafting", "g1").await;
    t.prompt(node, "g2");
    human_typed(&t.app, node);
    t.idle_request("drafting", "g1").await;
    let mark = t.mark();
    t.said("drafting", Some("g2")).await;
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    assert_eq!(t.relaunches(mark), []);
}

#[tokio::test(start_paused = true)]
async fn a_turn_end_that_something_answered_relaunches_nothing() {
    let t = fixture();
    let late = t.cursor("late");
    let newer = t.cursor("newer");
    let typed = t.cursor("typed");
    let drafted = t.cursor("drafted");
    let unchecked = t.cursor("unchecked");
    let exited = t.cursor("exited");
    let plain = t.cursor("plain");
    let moved = t.cursor("moved");
    let working = t.cursor("working");
    for node in [
        late, newer, typed, drafted, unchecked, exited, plain, moved, working,
    ] {
        t.heard(node, "g1");
    }
    let mark = t.mark();

    t.stop_hook("late", "g1").await;
    t.app.idle.open(late).finish(IdleEnd::Mail);
    t.said("late", Some("g1")).await;

    t.prompt(newer, "g2");
    t.said("newer", Some("g1")).await;

    t.said("typed", Some("g1")).await;

    human_typed(&t.app, drafted);
    t.said("drafted", Some("g1")).await;

    t.idle_request("unchecked", "g1").await;
    t.said("unchecked", Some("g1")).await;

    t.app.graph.exited(exited, 0).unwrap();
    t.app.idle.end(exited, IdleEnd::Exit);
    t.said("exited", Some("g1")).await;

    t.said("plain", None).await;

    t.said("moved", Some("g1")).await;

    t.stop_hook("working", "g1").await;
    t.prompt(working, "g3");
    t.said("working", Some("g2")).await;

    tokio::time::sleep(turn_end::IDLE_GRACE / 2).await;
    human_typed(&t.app, typed);
    t.prompt(moved, "g2");
    tokio::time::sleep(turn_end::IDLE_GRACE * 2).await;
    assert_eq!(t.relaunches(mark), []);
    assert!(t.app.busy(newer), "the newer turn still runs");
    assert!(t.app.busy(working), "an unseen old turn does not end g3");
}

#[tokio::test]
async fn a_question_box_turn_with_a_loom_ask_is_not_asked_again() {
    let t = fixture();
    let ask = AskRequest {
        question: "배포할까요? (A: 예 / B: 아니오, 권장: A)".into(),
        doc: None,
    };
    let Json(_) = agent_ask(t.state(), headers(LEAD, "ask"), Json(ask))
        .await
        .unwrap();
    let Json(()) = agent_question_box(t.state(), headers(LEAD, "question-box"))
        .await
        .unwrap();
    assert_eq!(t.app.turn_end(t.lead), None);

    let Json(()) = agent_question_box(t.state(), headers(LEAD, "question-box"))
        .await
        .unwrap();
    t.app.turn_ends.called(t.lead);
    let mark = t.mark();
    assert!(t.app.turn_end(t.lead).is_some());
    t.wrote("turn end", mark, |e| {
        matches!(e, Entry::Nudged { node, check: TurnCheck::QuestionBox } if node.id == Some(t.lead))
    });
}

#[test]
fn old_message_verbs_are_gone() {
    for old in ["send", "steer", "interrupt", "reply", "merge"] {
        assert!(
            crate::Cli::try_parse_from(["loom", old, "a", "b"]).is_err(),
            "{old}"
        );
    }
}

#[test]
fn spawn_takes_a_role_and_never_an_agent_model_or_effort() {
    let parse =
        |args: &[&str]| crate::Cli::try_parse_from(["loom", "spawn"].iter().chain(args).copied());
    for flags in [
        &["w", "--agent", "claude"][..],
        &["w", "--role", "워커", "--model", "m"],
        &["w", "--role", "워커", "--effort", "high"],
        &["w"],
        &["w", "--role", "검토자", "--review", "b"],
        &["--role", "검토자", "--review", "b", "--worktree", "x"],
    ] {
        assert!(parse(flags).is_err(), "{flags:?}");
    }
    for flags in [
        &["w", "--role", "워커"][..],
        &["--role", "검토자", "--review", "b"],
        &["s", "--", "ls"],
    ] {
        assert!(parse(flags).is_ok(), "{flags:?}");
    }
    let review = SpawnRun::Review {
        role: "검토자 ".into(),
        branch: " 기계화".into(),
    };
    assert_eq!(review.name(), "검토자 기계화");
}
