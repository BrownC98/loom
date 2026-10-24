use super::*;

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/waits/{wait}", delete(dismiss_wait))
        .route("/api/waits/{wait}/answer", post(answer_wait))
        .route("/api/waits/{wait}/choose", post(choose_wait))
        .route("/api/waits/{wait}/type", post(type_wait))
        .route("/api/docs", get(list_docs))
        .route(
            "/api/docs/{id}",
            post(open_doc).layer(axum::middleware::from_fn(origin::from_page)),
        )
        .route("/api/docs/{id}/", get(doc))
        .route("/api/docs/{id}/{*file}", get(doc_sibling))
        .route("/api/waits/{wait}/images/{index}", get(done_image_file))
}

/// 사람이 화면에서 `loom ask` 질문에 쓴 답. `images`는 답에 붙인 그림의 데몬 기계 경로다(`POST /api/pastes`가 준 값, 목업 M30).
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct AnswerWait {
    pub text: String,
    #[serde(default)]
    pub images: Vec<PathBuf>,
}

/// 사람이 알림에서 고른 승인 창 선택지. `prompt`는 화면이 보인 창 그대로, `option`은 그 `options`의 차례다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct ChooseOption {
    pub prompt: Prompt,
    #[ts(type = "number")]
    pub option: usize,
}

/// 사람이 알림의 입력칸에 쓴 글. 데몬은 `prompt`가 지금 화면과 같을 때만 그 글과 Enter를 치고, 글을 저장하거나 로그에 쓰지 않는다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct TypeText {
    pub prompt: Prompt,
    pub text: String,
}

async fn dismiss_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.dismiss(WaitId(wait))?;
    Ok(Json(()))
}

async fn answer_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<AnswerWait>,
) -> Result<Json<()>, Problem> {
    let node = app.graph.answer(WaitId(wait), &req.text, &req.images)?;
    app.turn_ends.human(node);
    Ok(Json(()))
}

async fn choose_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<ChooseOption>,
) -> Result<Json<()>, Problem> {
    let wait = WaitId(wait);
    let (node, _) = app.graph.dialog(wait)?;
    let server = app.opencode2.lock().unwrap().get(&node).cloned();
    match server {
        Some(server) => {
            let decision = server
                .decide(&req.prompt, req.option)
                .ok_or(Refusal::PromptChanged(wait))?;
            server.reply(decision).await.map_err(internal)?;
            match app.graph.dismiss(wait) {
                Ok(()) | Err(Refusal::NoWait(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        None => choose(&app.graph, &app.host, wait, &req)?,
    }
    Ok(Json(()))
}

fn choose(
    graph: &Graph,
    host: &NodeHost<NodeId>,
    wait: WaitId,
    req: &ChooseOption,
) -> Result<(), Refusal> {
    answer_dialog(graph, host, wait, &req.prompt, |now| {
        now.options.get(req.option)?.key.bytes()
    })
}

async fn type_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<TypeText>,
) -> Result<Json<()>, Problem> {
    type_text(&app.graph, &app.host, WaitId(wait), &req)?;
    Ok(Json(()))
}

fn type_text(
    graph: &Graph,
    host: &NodeHost<NodeId>,
    wait: WaitId,
    req: &TypeText,
) -> Result<(), Refusal> {
    answer_dialog(graph, host, wait, &req.prompt, |now| {
        now.field.map(|_| entered(&req.text))
    })
}

fn answer_dialog(
    graph: &Graph,
    host: &NodeHost<NodeId>,
    wait: WaitId,
    shown: &Prompt,
    keys: impl FnOnce(&Prompt) -> Option<Vec<u8>>,
) -> Result<(), Refusal> {
    let (node, agent) = graph.dialog(wait)?;
    let profile = agent.as_ref().and_then(profile::profile_of);
    host.write_if(node, |screen| {
        let now = screen.screen();
        let keys = profile::prompt(profile, &now)
            .filter(|now| now == shown)
            .and_then(|now| keys(&now))
            .ok_or(Refusal::PromptChanged(wait))?;
        graph.dismiss(wait)?;
        Ok(keys)
    })
}

async fn list_docs() -> Result<Json<Vec<user_doc::DocRow>>, Problem> {
    Ok(Json(user_doc::docs().map_err(internal)?))
}

async fn open_doc(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<()>, DocProblem> {
    doc_file(&id)?;
    open::open_url(&format!("http://127.0.0.1:{}/api/docs/{id}/", app.port)).map_err(internal)?;
    Ok(Json(()))
}

async fn doc(
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> Result<Response, DocProblem> {
    let doc = doc_file(&id)?;
    Ok(serve_file(&doc.path, request).await?)
}

async fn doc_sibling(
    Path((id, file)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Response, DocProblem> {
    let doc = doc_file(&id)?;
    match doc.folder.and_then(|dir| passes::inside(&dir, &file)) {
        Some(path) => Ok(serve_file(&path, request).await?),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

pub enum DocProblem {
    Missing(String),
    Other(Problem),
}

impl From<Problem> for DocProblem {
    fn from(e: Problem) -> Self {
        DocProblem::Other(e)
    }
}

impl IntoResponse for DocProblem {
    fn into_response(self) -> Response {
        match self {
            DocProblem::Missing(reason) => (
                StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, TEXT_TYPE)],
                reason,
            )
                .into_response(),
            DocProblem::Other(problem) => problem.into_response(),
        }
    }
}

pub(super) fn doc_file(id: &str) -> Result<user_doc::DocFile, DocProblem> {
    user_doc::doc_file(id).map_err(DocProblem::Missing)
}

async fn done_image_file(
    State(app): State<Arc<App>>,
    Path((wait, index)): Path<(u32, usize)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let path = app.graph.done_image(WaitId(wait), index)?;
    serve_file(&path, request).await
}

const TEXT_TYPE: &str = "text/plain; charset=utf-8";

pub(super) async fn serve_file(
    path: &std::path::Path,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    Ok(tower_http::services::ServeFile::new(path)
        .try_call(request)
        .await
        .map_err(internal)?
        .into_response())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_choice_types_once_and_stale_choices_type_nothing() {
        const SCREEN: &str = "──────\n Bash command\n╌╌╌╌\n touch one\n╌╌╌╌\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n\n";
        let dir = tempfile::tempdir().unwrap();
        let db = db::open(&dir.path().join("loom.db")).unwrap();
        db.lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let graph = Graph::open(db).unwrap();
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let launched = Launched {
            token: "t".into(),
            server: None,
            account: None,
            rules: None,
        };
        let node = graph
            .add_root(
                &place,
                Some(crate::protocol::AgentId("claude".into())),
                None,
                &launched,
                |_| Ok(()),
            )
            .unwrap();
        let screen_cmd = || {
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                &format!("stty -icanon -echo; printf '{SCREEN}'; exec cat"),
            ]);
            cmd
        };
        let hosts = dir.path().join("hosts");
        node_host::in_process(&hosts, node, screen_cmd());
        let host = NodeHost::new(hosts.clone());
        let (exited, exit) = std::sync::mpsc::channel();
        host.resume(node, move |_| exited.send(()).unwrap())
            .unwrap();
        let shown = loop {
            if let Some(prompt) = profile::prompt(Some(Profile::Claude), &host.screen(node)) {
                break prompt;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut typed = host.attach(node).unwrap().output;
        let open = |token: &str| {
            graph
                .set_wait(
                    token,
                    WaitSource::Hook,
                    Some((WaitKind::Approval, "permission")),
                )
                .unwrap();
            let views = graph.subscribe().unwrap().0;
            let view = views.into_iter().find(|v| v.id == node).unwrap();
            view.waits.iter().map(|w| w.id).collect::<Vec<_>>()
        };
        let pick = |wait, prompt: &Prompt| {
            choose(
                &graph,
                &host,
                wait,
                &ChooseOption {
                    prompt: prompt.clone(),
                    option: 0,
                },
            )
        };
        let typed_until_mark = |typed: &mut tokio::sync::broadcast::Receiver<Bytes>| {
            host.write_if(node, |_| Ok::<_, std::convert::Infallible>(b"Z".to_vec()))
                .unwrap();
            let mut seen = String::new();
            while !seen.ends_with('Z') {
                seen.push_str(&String::from_utf8_lossy(&typed.blocking_recv().unwrap()));
            }
            seen
        };

        let wait = open("t")[0];
        let barrier = std::sync::Barrier::new(2);
        let results: Vec<_> = std::thread::scope(|scope| {
            let both: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        pick(wait, &shown)
                    })
                })
                .collect();
            both.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert!(results.contains(&Ok(())), "{results:?}");
        assert!(results.contains(&Err(Refusal::NoWait(wait))), "{results:?}");

        let wait = open("t")[0];
        graph.typed(node).unwrap();
        assert_eq!(pick(wait, &shown), Err(Refusal::NoWait(wait)));

        let wait = open("t")[0];
        let mut older = shown.clone();
        older.rows[3] = " touch two".into();
        assert_eq!(pick(wait, &older), Err(Refusal::PromptChanged(wait)));
        assert_eq!(graph.dialog(wait).unwrap().0, node);

        assert_eq!(typed_until_mark(&mut typed), "1Z");

        host.kill(node);
        exit.recv().unwrap();
        let relaunched = Launched {
            token: "t2".into(),
            server: None,
            account: None,
            rules: None,
        };
        graph.relaunched(node, &relaunched).unwrap();
        node_host::in_process(&hosts, node, screen_cmd());
        host.resume(node, |_| {}).unwrap();
        while profile::prompt(Some(Profile::Claude), &host.screen(node)).as_ref() != Some(&shown) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut typed = host.attach(node).unwrap().output;
        assert_eq!(pick(wait, &shown), Err(Refusal::NoWait(wait)));
        assert_eq!(open("t"), vec![]);
        let newer = open("t2")[0];
        assert_eq!(pick(wait, &shown), Err(Refusal::NoWait(wait)));
        assert_eq!(graph.dialog(newer).unwrap().0, node);
        assert_eq!(typed_until_mark(&mut typed), "Z");
        host.kill(node);
    }

    #[cfg(unix)]
    struct Dialogs {
        dir: tempfile::TempDir,
        graph: Graph,
        host: NodeHost<NodeId>,
        count: usize,
    }

    #[cfg(unix)]
    impl Dialogs {
        fn new() -> Dialogs {
            let dir = tempfile::tempdir().unwrap();
            let db = db::open(&dir.path().join("loom.db")).unwrap();
            db.lock()
                .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
                .unwrap();
            let host = NodeHost::new(dir.path().join("hosts"));
            Dialogs {
                graph: Graph::open(db).unwrap(),
                host,
                dir,
                count: 0,
            }
        }

        fn start(&mut self, agent: Option<&str>, script: &str) -> (NodeId, String) {
            self.count += 1;
            let token = format!("t{}", self.count);
            let launched = Launched {
                token: token.clone(),
                server: None,
                account: None,
                rules: None,
            };
            let place = Place {
                repo: RepoId(1),
                worktree: "/repo".into(),
            };
            let node = self
                .graph
                .add_root(
                    &place,
                    agent.map(|a| crate::protocol::AgentId(a.into())),
                    None,
                    &launched,
                    |_| Ok(()),
                )
                .unwrap();
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                &format!(
                    "stty -icanon -echo -icrnl; until [ \"$(stty size)\" = '40 120' ]; do sleep 0.02; done; {script}"
                ),
            ]);
            node_host::in_process(&self.dir.path().join("hosts"), node, cmd);
            self.host.resume(node, |_| {}).unwrap();
            self.host.resize(node, 120, 40);
            (node, token)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn until(&self, node: NodeId, mut done: impl FnMut(&crate::screen::Screen) -> bool) {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !done(&self.host.screen(node)) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{}",
                    self.host.screen(node).text
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn shown(&self, node: NodeId, token: &str, profile: Option<Profile>) -> (WaitId, Prompt) {
            let mut found = None;
            self.until(node, |now| {
                found = profile::wait(profile, now, crate::screen::Session::NotStarted)
                    .zip(profile::prompt(profile, now));
                found.is_some()
            });
            let ((kind, text), prompt) = found.unwrap();
            self.graph
                .set_wait(token, WaitSource::Screen, Some((kind, &text)))
                .unwrap();
            self.graph.set_prompt(node, Some(prompt.clone())).unwrap();
            let view = self
                .graph
                .subscribe()
                .unwrap()
                .0
                .into_iter()
                .find(|v| v.id == node)
                .unwrap();
            assert_eq!(view.waits[0].prompt.as_ref(), Some(&prompt));
            (view.waits[0].id, prompt)
        }
    }

    #[cfg(unix)]
    #[test]
    fn dialog_buttons_and_fields_type_into_the_terminal_once() {
        let mut t = Dialogs::new();
        let secret = "hunter2-loom";
        let cases: [(Option<&str>, &str, Answer, &str); 9] = [
            (
                Some("claude"),
                include_str!("../profile/screens/claude-trust.txt"),
                Answer::Option(1, b"\x1b[B\r"),
                "clear",
            ),
            (
                Some("claude"),
                include_str!("../profile/screens/claude-login.txt"),
                Answer::Option(0, b"1"),
                "clear",
            ),
            (
                Some("codex"),
                include_str!("../profile/screens/codex-trust.txt"),
                Answer::Option(0, b"1"),
                "clear",
            ),
            (
                Some("cursor"),
                include_str!("../profile/screens/cursor-trust.txt"),
                Answer::Option(0, b"a"),
                "clear",
            ),
            (
                None,
                "$ apt remove x\nDo you want to continue? [Y/n] ",
                Answer::Option(1, b"n\r"),
                "newline",
            ),
            (
                None,
                "$ sudo ls\n[sudo] password for me: ",
                Answer::Text(secret),
                "newline",
            ),
            (
                None,
                "$ read -p 'Enter password: ' p\nEnter password: ",
                Answer::Text(secret),
                "echo",
            ),
            (
                None,
                "Username for 'https://example.com': ",
                Answer::Text("me"),
                "newline",
            ),
            (
                None,
                "Reset password? [y/n] ",
                Answer::Option(0, b"y\r"),
                "newline",
            ),
        ];
        for (i, (agent, screen_text, answer, after)) in cases.into_iter().enumerate() {
            let fixture = t.file(&format!("screen{i}"));
            let typed = t.file(&format!("typed{i}"));
            std::fs::write(&fixture, screen_text.trim_end_matches('\n')).unwrap();
            let expected: Vec<u8> = match answer {
                Answer::Option(_, keys) => keys.to_vec(),
                Answer::Text(text) => format!("{text}\r").into_bytes(),
            };
            let (read, then) = match after {
                "clear" => ("", r"printf '\033[2J\033[Hanswered'"),
                "newline" => ("", r"printf '\n'"),
                _ => ("stty echo; ", "true"),
            };
            let (node, token) = t.start(
                agent,
                &format!(
                    "cat '{}'; {read}head -c {} > '{}'; {then}; exec cat",
                    fixture.display(),
                    expected.len(),
                    typed.display()
                ),
            );
            let profile = agent
                .map(|a| crate::protocol::AgentId(a.into()))
                .as_ref()
                .and_then(profile::profile_of);
            let (wait, shown) = t.shown(node, &token, profile);
            let sent = match answer {
                Answer::Option(option, _) => choose(
                    &t.graph,
                    &t.host,
                    wait,
                    &ChooseOption {
                        prompt: shown.clone(),
                        option,
                    },
                ),
                Answer::Text(text) => {
                    let option = ChooseOption {
                        prompt: shown.clone(),
                        option: 0,
                    };
                    assert_eq!(
                        choose(&t.graph, &t.host, wait, &option),
                        Err(Refusal::PromptChanged(wait))
                    );
                    let req = TypeText {
                        prompt: shown.clone(),
                        text: text.into(),
                    };
                    type_text(&t.graph, &t.host, wait, &req)
                }
            };
            assert_eq!(sent, Ok(()), "{i}");
            assert_eq!(t.graph.dialog(wait), Err(Refusal::NoWait(wait)), "{i}");
            t.until(node, |_| std::fs::read(&typed).is_ok_and(|b| b == expected));
            let settled = |now: &crate::screen::Screen| match after {
                "clear" => now.text.contains("answered"),
                "newline" => now.to_cursor.ends_with('\n'),
                _ => now.text.contains(secret),
            };
            t.until(node, settled);
            let now = t.host.screen(node);
            assert_eq!(profile::prompt(profile, &now), None, "{i}");
            assert_eq!(
                profile::wait(profile, &now, crate::screen::Session::NotStarted),
                None,
                "{i}"
            );
            t.host.kill(node);
        }
        let Dialogs { dir, graph, .. } = t;
        drop(graph);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() && !path.ends_with("typed5") && !path.ends_with("typed6") {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                    "{path:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_field_types_nothing_when_the_lines_above_the_prompt_change() {
        let mut t = Dialogs::new();
        let next = t.file("next");
        let typed = t.file("typed");
        let (node, token) = t.start(
            None,
            &format!(
                "printf 'Account: LOCAL\\nPassword: '; until [ -e '{next}' ]; do sleep 0.02; done; printf '\\033[2J\\033[HAccount: REMOTE\\nPassword: '; head -c 3 > '{typed}'; exec cat",
                next = next.display(),
                typed = typed.display()
            ),
        );
        let (wait, local) = t.shown(node, &token, None);
        std::fs::write(&next, "").unwrap();
        t.until(node, |now| now.text.contains("REMOTE"));
        let req = |prompt: &Prompt| TypeText {
            prompt: prompt.clone(),
            text: "pw".into(),
        };
        assert_eq!(
            type_text(&t.graph, &t.host, wait, &req(&local)),
            Err(Refusal::PromptChanged(wait))
        );
        let (wait, remote) = t.shown(node, &token, None);
        assert_ne!(local, remote);
        assert_eq!(type_text(&t.graph, &t.host, wait, &req(&remote)), Ok(()));
        t.until(node, |_| std::fs::read(&typed).is_ok_and(|b| b == b"pw\r"));
        t.host.kill(node);
    }

    #[cfg(unix)]
    enum Answer {
        Option(usize, &'static [u8]),
        Text(&'static str),
    }
}
