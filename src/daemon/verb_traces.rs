use clap::{CommandFactory, Parser};
use tempfile::TempDir;

use super::commands::{
    agent_guard, agent_idle, agent_interrupt_pending, agent_question_box, agent_take,
    agent_turn_end,
};
use super::*;
use crate::graph::{AgentState, Entry, Round, TurnCheck};
use crate::protocol::{AgentId, Judgment};

const UNTRACED: [(&str, &str); 17] = [
    ("serve", "runs the daemon"),
    ("start", "starts or joins the daemon"),
    ("open", "opens the screen"),
    ("stop", "stops the daemon"),
    ("carry", "moves an old daemon's data"),
    (
        "roles",
        "reads the settings; `roles set` leaves a refusal event and view-changed (see roles_set_*)",
    ),
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
    ("pi-hook", "hook transport that the agent CLI runs"),
    ("guard", "hook transport that the agent CLI runs"),
    ("pty-host", "the node process host"),
    (
        "tunnel",
        "the WSL relay that carries hook connections to the daemon",
    ),
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
            crate::tunnel::Door::new().0,
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
            branched_from: None,
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
                keep: Vec::new(),
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
            t.app.feed(t.worker, AgentEvent::WaitOpened);
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
        .said(t.lead, "A안과 B안 가운데 고르시면 진행합니다.".into(), None);
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
    fn idle(
        &self,
        token: &str,
        turn_end: Option<EndedTurn>,
    ) -> impl Future<Output = Result<Json<IdleReply>, AgentError>> + Send + 'static {
        let req = IdleRequest {
            turn_end,
            ..IdleRequest::default()
        };
        agent_idle(self.state(), headers(token, "idle"), Json(Some(req)))
    }

    async fn idle_request(&self, token: &str) {
        let _ = tokio::time::timeout(Duration::from_millis(1), self.idle(token, None)).await;
    }

    async fn held_idle_request(
        &self,
        token: &str,
        turn_end: Option<EndedTurn>,
    ) -> tokio::task::JoinHandle<Option<IdleReply>> {
        let idle = self.idle(token, turn_end);
        let held = tokio::spawn(async move { idle.await.ok().map(|Json(reply)| reply) });
        tokio::time::sleep(Duration::from_millis(1)).await;
        held
    }

    async fn tool_hook(&self, token: &str) {
        let req = TakeRequest {
            hook: "postToolUse".into(),
            kinds: vec![],
            limit_bytes: 8192,
            transcript: None,
            human_input: false,
            prompt: None,
            turn: None,
        };
        ok(
            "take",
            agent_take(self.state(), headers(token, "take"), Json(req)).await,
        );
    }

    async fn stop_hook(&self, token: &str) {
        ok(
            "turn-end",
            agent_turn_end(self.state(), headers(token, "turn-end")).await,
        );
    }
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

fn set(role: &str, agent: &str, model: Option<&str>, effort: Option<&str>) -> Json<SetRole> {
    Json(SetRole {
        role: role.into(),
        agent: AgentId(agent.into()),
        model: model.map(Into::into),
        effort: effort.map(Into::into),
    })
}

#[tokio::test]
async fn roles_set_changes_one_role_and_publishes_view_changed() {
    let t = fixture();
    let (_, mut events) = t.app.graph.subscribe().unwrap();
    let Json(got) = agent_set_role(
        t.state(),
        headers(LEAD, "roles"),
        set("워커", "codex", Some("m"), None),
    )
    .await
    .unwrap();
    let before = Roles::default();
    let worker = got.0.iter().find(|r| r.name == "워커").unwrap();
    assert_eq!(worker.agent, Some(AgentId("codex".into())));
    assert_eq!(worker.model.as_deref(), Some("m"));
    assert_eq!(worker.effort, None);
    assert_eq!(worker.duty, before.0[2].duty);
    assert_eq!(got.0.len(), before.0.len());
    assert_eq!(t.app.settings.get().unwrap().roles, got);
    assert!(matches!(
        events.try_recv(),
        Ok(crate::graph::GraphEvent::ViewChanged)
    ));
}

#[tokio::test]
async fn roles_set_refuses_a_child_an_unknown_role_and_an_unknown_agent() {
    let t = fixture();
    let (_, mut events) = t.app.graph.subscribe().unwrap();
    let mark = t.mark();
    let refused = agent_set_role(
        t.state(),
        headers(WORKER, "roles"),
        set("워커", "claude", None, None),
    )
    .await;
    assert_eq!(t.refused("roles", mark, refused), Refusal::RolesHaveParent);

    let mark = t.mark();
    let refused = agent_set_role(
        t.state(),
        headers(LEAD, "roles"),
        set("없는역할", "claude", None, None),
    )
    .await;
    assert!(matches!(
        t.refused("roles", mark, refused),
        Refusal::UnknownRole { .. }
    ));

    let mark = t.mark();
    let refused = agent_set_role(
        t.state(),
        headers(LEAD, "roles"),
        set("워커", "nobody", None, None),
    )
    .await;
    assert!(matches!(
        t.refused("roles", mark, refused),
        Refusal::BadRoleSetting(_)
    ));

    let mark = t.mark();
    let refused = agent_set_role(
        t.state(),
        headers(LEAD, "roles"),
        set("워커", "cursor", None, Some("high")),
    )
    .await;
    assert!(matches!(
        t.refused("roles", mark, refused),
        Refusal::BadRoleSetting(_)
    ));

    assert_eq!(t.app.settings.get().unwrap().roles, Roles::default());
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, crate::graph::GraphEvent::ViewChanged));
    }
}

#[test]
fn roles_set_needs_an_agent_and_roles_alone_still_parses() {
    let parse =
        |args: &[&str]| crate::Cli::try_parse_from(["loom", "roles"].iter().chain(args).copied());
    assert!(parse(&[]).is_ok());
    assert!(parse(&["set", "워커"]).is_err());
    assert!(parse(&["set", "워커", "--effort", "high"]).is_err());
    assert!(parse(&["set", "워커", "--agent", "claude"]).is_ok());
    assert!(
        parse(&[
            "set", "워커", "--agent", "claude", "--model", "m", "--effort", "high"
        ])
        .is_ok()
    );
}

#[tokio::test]
async fn guard_records_no_host_process_for_a_wsl_node() {
    let t = fixture();
    let mut cmd = match cfg!(windows) {
        true => portable_pty::CommandBuilder::new("powershell.exe"),
        false => portable_pty::CommandBuilder::new("sh"),
    };
    cmd.args(match cfg!(windows) {
        true => [
            "-NoProfile",
            "-Command",
            "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; Write-Output \"pid=$($p.Id)=\"; $p.WaitForExit()",
        ]
        .as_slice(),
        false => ["-c", "sleep 60 & echo pid=$!=; wait"].as_slice(),
    });
    node_host::in_process(&t.dir.path().join("pty").join("nodes"), t.worker, cmd);
    t.app.host.resume(t.worker, |_| {}).unwrap();
    let child: u32 = loop {
        let text = t.app.host.screen(t.worker).text;
        if let Some(pid) = text
            .split_once("pid=")
            .and_then(|(_, rest)| rest.split_once('='))
            .and_then(|(pid, _)| pid.trim().parse().ok())
        {
            break pid;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let guard = || {
        agent_guard(
            t.state(),
            headers(WORKER, "guard"),
            Json(GuardRequest { caller: child }),
        )
    };
    let place = |worktree: &str| {
        rusqlite::Connection::open(t.dir.path().join("loom.db"))
            .unwrap()
            .execute(
                "UPDATE nodes SET worktree = ?2 WHERE id = ?1",
                rusqlite::params![t.worker.0, worktree],
            )
            .unwrap();
    };
    if cfg!(windows) {
        place(r"\\wsl.localhost\Ubuntu\home\me\repo");
        ok("guard", guard().await);
        assert_eq!(t.app.host.stop_guarded(t.worker), 0);
        place(&t.place.worktree);
    }
    ok("guard", guard().await);
    assert!(t.app.host.stop_guarded(t.worker) >= 1);
    t.app.host.kill(t.worker);
}

#[tokio::test]
async fn an_interrupt_check_leaves_the_idle_wait_open_and_the_mail_undelivered() {
    let t = fixture();
    let target = format!("#{}", t.worker.0);
    let pending = || agent_interrupt_pending(t.state(), headers(WORKER, "interrupt-pending"));
    assert!(!pending().await.unwrap().0);
    t.app
        .graph
        .note(t.lead, &target, Kind::Queue, "큐", None)
        .unwrap();
    assert!(!pending().await.unwrap().0);
    let _wait = t.app.idle.open(t.worker);
    t.app
        .graph
        .interrupt(t.lead, &target, "멈춰라", None, |_| {})
        .unwrap();
    assert!(pending().await.unwrap().0);
    assert!(t.app.idle.waiting(t.worker));
    let taken = t.app.graph.take(t.worker, Kind::ALL, 8192).unwrap();
    assert_eq!(
        taken.iter().map(|d| d.body.as_str()).collect::<Vec<_>>(),
        ["큐", "멈춰라"]
    );
    t.app
        .graph
        .ack(t.worker, &taken.iter().map(|d| d.id).collect::<Vec<_>>())
        .unwrap();
    assert!(!pending().await.unwrap().0);
}

impl Fixture {
    fn agent_state(&self, node: NodeId) -> Option<AgentState> {
        self.app.graph.agent_state(node).unwrap()
    }

    async fn settled(&self, node: NodeId, want: AgentState) {
        for _ in 0..100 {
            if self.agent_state(node).as_ref() == Some(&want) {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "node {node} stays {:?}, not {want:?}",
            self.agent_state(node)
        );
    }

    fn agent_changes(&self, mark: u32) -> Vec<(AgentState, AgentEvent)> {
        self.since(mark)
            .into_iter()
            .filter_map(|e| match e {
                Entry::AgentStateChanged { to, by, .. } => Some((to, by)),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn mail_handed_by_the_idle_wait_starts_a_turn() {
    let t = fixture();
    let held = t.held_idle_request(WORKER, None).await;
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Listening));
    t.app
        .graph
        .note(
            t.lead,
            &format!("#{}", t.worker.0),
            Kind::Queue,
            "일해",
            None,
        )
        .unwrap();
    held.await.unwrap();
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Turn));
    let mark = t.mark();
    assert!(t.app.busy(t.worker), "the handed mail starts a turn");
    assert_eq!(t.since(mark), []);
    assert_eq!(
        t.agent_changes(0),
        [
            (AgentState::Listening, AgentEvent::WaitOpened),
            (
                AgentState::Turn,
                AgentEvent::WaitClosed { by: IdleEnd::Mail }
            ),
        ]
    );
}

#[tokio::test]
async fn a_dropped_idle_wait_leaves_the_node_deaf_until_a_wait_opens() {
    let t = fixture();
    let mark = t.mark();
    t.idle_request(WORKER).await;
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Deaf));
    let held = t.held_idle_request(WORKER, None).await;
    let newer = t.held_idle_request(WORKER, None).await;
    held.abort();
    let _ = held.await;
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Listening));
    t.tool_hook(WORKER).await;
    let _ = newer.await;
    t.stop_hook(WORKER).await;
    let gone = || AgentEvent::WaitClosed { by: IdleEnd::Gone };
    assert_eq!(
        t.agent_changes(mark),
        [
            (AgentState::Listening, AgentEvent::WaitOpened),
            (AgentState::Deaf, gone()),
            (AgentState::Listening, AgentEvent::WaitOpened),
            (AgentState::Turn, AgentEvent::TurnStarted),
            (AgentState::Deaf, AgentEvent::TurnEnded),
        ]
    );
}

#[tokio::test]
async fn server_status_edges_move_the_agent_state() {
    let t = fixture();
    let mark = t.mark();
    let (status, seen) = tokio::sync::watch::channel(None);
    let follow = tokio::spawn(runtime::follow_server_turns(
        t.app.clone(),
        t.worker,
        seen,
        |s: &Option<bool>| *s,
        |_: &App| true,
    ));
    tokio::task::yield_now().await;
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Starting));
    status.send_replace(Some(false));
    t.settled(t.worker, AgentState::Listening).await;
    status.send_replace(Some(true));
    t.settled(t.worker, AgentState::Turn).await;
    status.send_replace(Some(true));
    tokio::task::yield_now().await;
    status.send_replace(Some(false));
    t.settled(t.worker, AgentState::Listening).await;
    drop(status);
    follow.await.unwrap();
    assert_eq!(
        t.agent_changes(mark),
        [
            (AgentState::Listening, AgentEvent::ServerIdle),
            (AgentState::Turn, AgentEvent::ServerBusy),
            (AgentState::Listening, AgentEvent::ServerIdle),
            (AgentState::Deaf, AgentEvent::ServerLost),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn an_old_process_that_does_not_exit_gives_back_its_state_with_the_events_since() {
    let t = fixture();
    let mut wait = t.app.idle.open(t.worker);
    let mark = t.mark();
    let node = t
        .app
        .graph
        .active()
        .unwrap()
        .into_iter()
        .find(|n| n.id == t.worker)
        .unwrap();
    let app = t.app.clone();
    let ending = tokio::spawn(async move { app.end_old(&node, || {}, None).await });
    let by = wait.ended().await;
    wait.finish(by);
    let ended = ending.await.unwrap();
    assert!(ended.is_err(), "{:?}", ended.ok());
    let relaunching = |before| AgentState::Relaunching {
        before: Box::new(before),
    };
    assert_eq!(
        t.agent_changes(mark),
        [
            (
                relaunching(AgentState::Listening),
                AgentEvent::RelaunchStarted
            ),
            (
                relaunching(AgentState::Deaf),
                AgentEvent::WaitClosed {
                    by: IdleEnd::Relaunch
                }
            ),
            (AgentState::Deaf, AgentEvent::RelaunchFailed),
        ]
    );
}

#[tokio::test]
async fn a_kept_relaunch_releases_the_held_idle_wait_and_its_reopen_listens_again() {
    let t = fixture();
    t.app.hold(t.worker);
    let mark = t.mark();
    let held = t.held_idle_request(WORKER, None).await;
    t.app.kept_running(t.worker);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(t.app.idle.waiting(t.worker), "the wait reopened");
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Listening));
    assert_eq!(
        t.agent_changes(mark),
        [
            (AgentState::Listening, AgentEvent::WaitOpened),
            (
                AgentState::Deaf,
                AgentEvent::WaitClosed {
                    by: IdleEnd::Relaunch
                }
            ),
            (AgentState::Listening, AgentEvent::WaitOpened),
        ]
    );
    held.abort();
}

#[tokio::test]
async fn a_turn_end_that_opens_its_idle_wait_goes_from_turn_to_listening_in_one_step() {
    let t = fixture();
    t.tool_hook(WORKER).await;
    let mark = t.mark();
    let nudged = t
        .held_idle_request(
            WORKER,
            Some(EndedTurn {
                reply: Some("끝.\nQUESTION: 지울까?".into()),
                turn: None,
            }),
        )
        .await;
    assert!(matches!(
        nudged.await.unwrap(),
        Some(IdleReply::Nudge { .. })
    ));
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Turn));
    let held = t
        .held_idle_request(WORKER, Some(EndedTurn::default()))
        .await;
    assert_eq!(t.agent_state(t.worker), Some(AgentState::Listening));
    assert_eq!(
        t.agent_changes(mark),
        [(AgentState::Listening, AgentEvent::TurnEndedWaitOpened)]
    );
    held.abort();
    let _ = held.await;

    t.tool_hook(WORKER).await;
    let mark = t.mark();
    t.stop_hook(WORKER).await;
    let old = t.held_idle_request(WORKER, None).await;
    assert_eq!(
        t.agent_changes(mark),
        [
            (AgentState::Deaf, AgentEvent::TurnEnded),
            (AgentState::Listening, AgentEvent::WaitOpened),
        ],
        "a hook of the old binary still passes through deaf"
    );
    old.abort();
}
