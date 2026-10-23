//! 에이전트 동사마다 성공과 거절이 이벤트를 남기는지 본다. `loom audit` 명령 대신 이 시험이 추적을 강제한다(R14 D3 A, 2026-10-23 Q687 A).
//! CLI 명령 목록(`crate::Cli`)을 돌므로, 새 명령은 `scene`에 성공·거절 장면을 더하거나 `UNTRACED`에 까닭을 적어야 이 시험이 지난다.
//! 거절 장면은 모두 처리기를 지난다. 성공 장면 가운데 다섯은 처리기가 아니라 그 끝의 그래프 호출만 본다:
//! `spawn`(`add_child`, 띄우기에 PTY 호스트가 있어야 한다), `ready`·`verdict`·`merge`·`fail`(git 저장소와 관문 스크립트가 있어야 한다).
//! 그래서 이 다섯의 처리기가 성공 길에서 그래프를 부르지 않게 바뀌어도 이 시험은 잡지 못한다. `relaunch`의 성공 장면은 세션이 없어 실패한 다시 띄우기다.

use clap::{CommandFactory, Parser};
use tempfile::TempDir;

use super::*;
use crate::graph::{Entry, Round, TurnCheck};
use crate::protocol::{CI_ROLE, Judgment};

/// 이벤트를 남기지 않는 명령과 그 까닭.
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
    ci: NodeId,
}

const LEAD: &str = "lead-token";
const WORKER: &str = "worker-token";
const CI: &str = "ci-token";

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
        ci: lead,
    };
    fixture.worker = fixture.child("worker", None, WORKER);
    fixture.ci = fixture.child("ci", Some(CI_ROLE), CI);
    fixture
}

impl Fixture {
    fn child(&self, name: &str, role: Option<&str>, token: &str) -> NodeId {
        let child = NewChild {
            name,
            worktree: None,
            agent: claude(),
            brief: "",
            role,
            task: "t",
        };
        let limits = TreeLimits {
            max_depth: None,
            max_children: None,
        };
        self.app
            .graph
            .add_child(self.lead, child, limits, &launched(token), |_| Ok(()))
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

    /// 그 동사가 부른 쪽에게 거절을 돌려주었고, 같은 거절을 `Refused`로 남겼다.
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

/// 동사 하나의 거절 장면과 성공 장면.
async fn scene(t: &Fixture, verb: &str) {
    let mark = t.mark();
    match verb {
        "spawn" => {
            let req = SpawnRequest {
                name: "made".into(),
                run: SpawnRun::Agent(claude()),
                brief: "no heads".into(),
                worktree: None,
                choice: ModelChoice::default(),
                role: None,
            };
            let refused = agent_spawn(t.state(), headers(LEAD, verb), Json(req)).await;
            assert!(matches!(
                t.refused(verb, mark, refused),
                Refusal::MissingHeads(_)
            ));
            // 띄우기는 PTY 호스트 프로세스가 있어야 해서 시험에서 돌지 않는다. 성공의 흔적(`Created`)은 그 앞 단계 `add_child`가 쓴다.
            let mark = t.mark();
            t.child("made", None, "made-token");
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
            // 사람이 읽는 글 속 `file://` 링크는 `show.sh`를 지난 HTML만 받는다.
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
            t.child("spare", None, "spare-token");
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
            // 남긴 세션이 없는 Claude 노드라 다시 띄우지 못한다. 그 결과도 `Relaunched`로 남는다.
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
        // 관문 동사의 성공은 git 저장소와 관문 스크립트가 있어야 데몬 길로 돈다. 성공의 흔적은 그 끝 단계인 그래프 호출이 쓴다.
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
            t.app.graph.ready(t.worker, &round, t.ci, None).unwrap();
            t.wrote(verb, mark, message_from(t.worker));
        }
        "verdict" => {
            let req = gate::VerdictRequest {
                branch: "nope".into(),
                judgment: Judgment::Pass,
                findings: None,
            };
            let refused = gate::agent_verdict(t.state(), headers(CI, verb), Json(req)).await;
            t.refused(verb, mark, refused);
            let mark = t.mark();
            t.app
                .graph
                .verdict(t.ci, "b", Judgment::Pass, None, None)
                .unwrap();
            t.wrote(verb, mark, message_from(t.ci));
        }
        "merge" => {
            let req = gate::BranchRequest {
                branch: "nope".into(),
            };
            let refused = gate::agent_merge(t.state(), headers(CI, verb), Json(req)).await;
            t.refused(verb, mark, refused);
            let mark = t.mark();
            t.app.graph.merge_start(t.ci, "b", None).unwrap();
            t.wrote(verb, mark, message_from(t.ci));
        }
        "fail" => {
            let fail = |branch: &str| gate::FailRequest {
                branch: branch.into(),
                cause: "broken".into(),
            };
            let refused = gate::agent_fail(t.state(), headers(CI, verb), Json(fail("nope"))).await;
            t.refused(verb, mark, refused);
            let mark = t.mark();
            t.app.graph.fail(t.ci, "b", "broken", None).unwrap();
            t.wrote(verb, mark, message_from(t.ci));
        }
        _ => panic!(
            "{verb}: no trace scene. Add a success and a refusal scene here, or add {verb} to UNTRACED with the reason"
        ),
    }
}

/// 턴 끝 검사가 다시 일하게 하면 그 검사를 이벤트로 남긴다.
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
    // 다시 일하게 하지 않은 턴 끝은 남기지 않는다.
    let mark = t.mark();
    assert!(t.app.turn_end(t.lead).is_none());
    assert_eq!(t.since(mark), []);
}

/// 옛 메시지 동사는 지웠다(R14 D2 A).
#[test]
fn old_message_verbs_are_gone() {
    for old in ["send", "steer", "interrupt", "reply"] {
        assert!(
            crate::Cli::try_parse_from(["loom", old, "a", "b"]).is_err(),
            "{old}"
        );
    }
}
