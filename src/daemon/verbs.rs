use super::*;

pub(super) async fn agent_spawn(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SpawnRequest>,
) -> Result<Json<NodeId>, AgentError> {
    let parent = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/spawn");
    let name = req.name.clone();
    let spawned = spawn(&app, parent, req).await;
    traced(&app, parent, Attempt::verb(&name), &via, spawned)
}

async fn spawn(app: &Arc<App>, parent: NodeId, req: SpawnRequest) -> Result<NodeId, Problem> {
    let missing = task::missing_heads(&req.brief);
    if !req.brief.trim().is_empty() && !missing.is_empty() {
        return Err(
            Refusal::MissingHeads(missing.iter().map(|h| (*h).to_owned()).collect()).into(),
        );
    }
    let place = app.graph.place_of(parent).map_err(Refusal::from)?;
    let tasks = app.tasks_for(&place.worktree).await?;
    let (dir, fresh) = match &req.worktree {
        Some(branch) => {
            let base = task::base(&app.repos.path(place.repo)?, branch.trim());
            let (dir, fresh) = tasks.new_dir(&base).map_err(internal)?;
            (dir, Some(fresh))
        }
        None => (app.task_dir(parent).await?, None),
    };
    let given = req.brief.clone();
    let (start, queued_brief) = match req.run {
        SpawnRun::Agent(agent) => {
            let assigned = match &req.role {
                Some(role) => app
                    .settings
                    .get()
                    .map_err(internal)?
                    .roles
                    .assign(role, agent, req.choice, &req.brief)
                    .map_err(|e| match e {
                        RoleProblem::Unknown { role, names } => {
                            Refusal::UnknownRole { role, names }
                        }
                        RoleProblem::NoAgent { role } => Refusal::RoleHasNoAgent { role },
                    })?,
                None => Assignment {
                    agent: agent.ok_or(Refusal::NoAgentOrRole)?,
                    choice: req.choice,
                    brief: req.brief,
                },
            };
            (
                Start::Agent {
                    agent: assigned.agent,
                    brief: assigned.brief,
                    choice: assigned.choice,
                    resume: None,
                },
                String::new(),
            )
        }
        SpawnRun::Command(line) => (Start::Command { line }, req.brief),
    };
    let worktree = match &req.worktree {
        Some(branch) => {
            let repo = app.repos.path(place.repo)?;
            let workspace = app.workspace(&repo).await?;
            let from = std::path::Path::new(&place.worktree);
            let made = worktree::create(&repo, from, &workspace, branch, "HEAD").await?;
            Some((repo, made))
        }
        None => None,
    };
    let made = worktree.as_ref().map(|(_, made)| made.as_str());
    let cwd = std::path::Path::new(made.unwrap_or(&place.worktree));
    let child = NewChild {
        name: &req.name,
        worktree: made,
        agent: start.agent(),
        brief: &queued_brief,
        role: req.role.as_deref(),
        task: &dir.folder,
        review: req.review.as_deref(),
    };
    let spawned = spawn_child(app, parent, child, &start, cwd, &dir, &given).await;
    if let (Ok(_), Some(fresh)) = (&spawned, fresh) {
        fresh.keep();
    }
    if let (Some((repo, made)), Err(_)) = (&worktree, &spawned)
        && let Err(e) = worktree::discard(repo, made).await
    {
        eprintln!("loom: 띄우지 못한 자식의 워크트리 {made}를 지우지 못했다: {e}");
    }
    spawned
}

async fn spawn_child(
    app: &Arc<App>,
    parent: NodeId,
    child: NewChild<'_>,
    start: &Start,
    cwd: &std::path::Path,
    dir: &task::TaskDir,
    brief: &str,
) -> Result<NodeId, Problem> {
    let mut prepared = app.prepare(start, cwd, Position::Child, Some(dir)).await?;
    if child.worktree.is_some() {
        prepared.setup = worktree::setup_script(cwd).map(|line| prepared.shell(&line));
    }
    let discard = prepared.discarder();
    let limits = app.tree_limits().inspect_err(|_| discard())?;
    app.graph
        .add_child(parent, child, limits, &prepared.launched(), |id| {
            task::create(&dir.local, brief).map_err(|e| format!("{}: {e}", dir.local.display()))?;
            app.launch(id, prepared)
        })
        .map_err(Problem::from)
        .inspect_err(|_| discard())
}

pub(super) async fn agent_note(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<NoteRequest>,
) -> Result<Json<Sent>, AgentError> {
    let from = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/note");
    let sent = note(&app, from, &req, &via);
    recorded(&app, from, &req.to, req.mode, MessageKind::Note, &via, sent)
}

pub(super) fn recorded<T>(
    app: &App,
    from: NodeId,
    to: &str,
    how: Kind,
    what: MessageKind,
    via: &Via,
    result: Result<T, Refusal>,
) -> Result<Json<T>, AgentError> {
    if result.is_ok() {
        app.turn_ends.called(from);
    }
    let attempt = Attempt {
        to,
        how,
        what: Some(what),
    };
    traced(app, from, attempt, via, result.map_err(Problem::from))
}

pub(super) struct Attempt<'a> {
    to: &'a str,
    how: Kind,
    what: Option<MessageKind>,
}

impl<'a> Attempt<'a> {
    pub(super) fn verb(to: &'a str) -> Self {
        Attempt {
            to,
            how: Kind::Queue,
            what: None,
        }
    }
}

pub(super) fn traced<T>(
    app: &App,
    from: NodeId,
    attempt: Attempt,
    via: &Via,
    result: Result<T, Problem>,
) -> Result<Json<T>, AgentError> {
    if let Err(problem) = &result
        && let Some(reason) = refusal_of(problem)
    {
        app.graph
            .record_refusal(
                from,
                attempt.to,
                attempt.how,
                attempt.what,
                &reason,
                Some(via),
            )
            .map_err(|e| for_agent(Refusal::from(e)))?;
    }
    result.map(Json).map_err(for_agent)
}

fn refusal_of(problem: &Problem) -> Option<Refusal> {
    match problem {
        Problem::Refused(Refusal::Db(_)) | Problem::Internal(_) | Problem::NoLogin => None,
        Problem::Refused(reason) => Some(reason.clone()),
        Problem::Launch(e) => Some(Refusal::Launch(e.to_string())),
        Problem::Repo(e) => Some(Refusal::Git(e.to_string())),
    }
}

fn note(app: &App, from: NodeId, req: &NoteRequest, via: &Via) -> Result<Sent, Refusal> {
    app.kind_word(from, &req.body)?;
    if req.mode != Kind::Queue {
        let (to, agent) = app.graph.related(from, &req.to)?;
        let target = format!("#{}", to.0);
        let Some(profile) = agent.as_ref().and_then(profile::profile_of) else {
            return Err(Refusal::NotSteerable(req.to.clone()));
        };
        if req.mode == Kind::Interrupt && matches!(profile, Profile::Claude | Profile::Cursor) {
            return app
                .graph
                .interrupt(from, &target, &req.body, Some(via), |to| {
                    app.host.stop_guarded(to);
                });
        }
        return app
            .graph
            .note(from, &target, req.mode, &req.body, Some(via));
    }
    app.graph
        .note(from, &req.to, req.mode, &req.body, Some(via))
}

pub(super) async fn agent_answer(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AnswerRequest>,
) -> Result<Json<Sent>, AgentError> {
    let from = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/answer");
    let sent = app.kind_word(from, &req.body).and_then(|()| {
        app.graph
            .answer_ask(from, req.message, &req.body, Some(&via))
    });
    let to = format!("message #{}", req.message.0);
    let what = MessageKind::Answer {
        question: Question::Message { id: req.message.0 },
    };
    recorded(&app, from, &to, Kind::Queue, what, &via, sent)
}

pub(super) async fn agent_done(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<DoneRequest>,
) -> Result<Json<Reported>, AgentError> {
    let node = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/done");
    let what = MessageKind::Done {
        file: req.file.clone(),
        warning: None,
    };
    let checked = req
        .images
        .iter()
        .try_for_each(|image| done_image(image))
        .and_then(|()| {
            let parentless = app.graph.relations(node)?.parent.is_none();
            let unchecked = if parentless {
                unchecked_link(&app, node, &req.summary)?
            } else {
                None
            };
            match unchecked {
                Some(link) => Err(Refusal::UncheckedDoc(link)),
                None => attach(req.doc.as_deref()),
            }
        });
    let doc = match checked {
        Ok(doc) => doc,
        Err(refused) => {
            return recorded::<Reported>(&app, node, PARENT, Kind::Queue, what, &via, Err(refused));
        }
    };
    let word = app.kind_word(node, &req.summary);
    let warning = if word.is_ok() {
        app.unreadied(node).await.unwrap_or_else(|e| {
            eprintln!("loom: 노드 {}의 담당 브랜치를 보지 못했다: {e}", node.0);
            None
        })
    } else {
        None
    };
    let report = Report {
        summary: &req.summary,
        file: req.file.as_deref(),
        warning: warning.clone(),
    };
    let done = word.and_then(|()| {
        app.graph.done(
            node,
            &report,
            req.asked.as_deref(),
            &req.images,
            doc,
            Some(&via),
        )
    });
    let done = done.map(|alive| Reported {
        alive,
        warning: warning.clone(),
    });
    let what = MessageKind::Done {
        file: req.file.clone(),
        warning,
    };
    recorded(&app, node, PARENT, Kind::Queue, what, &via, done)
}

fn done_image(path: &std::path::Path) -> Result<(), Refusal> {
    let image = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
    if !image || !path.is_file() {
        return Err(Refusal::NotImage(path.display().to_string()));
    }
    Ok(())
}

fn attach(doc: Option<&std::path::Path>) -> Result<Option<WaitDoc>, Refusal> {
    doc.map(user_doc::attach).transpose()
}

const INBOX: &str = "inbox";

fn unchecked_link(app: &App, node: NodeId, text: &str) -> Result<Option<String>, Refusal> {
    let folder = app.graph.place_of(node)?.worktree;
    Ok(user_doc::unchecked_link(
        text,
        std::path::Path::new(&folder),
    ))
}

pub(super) async fn agent_ask(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AskRequest>,
) -> Result<Json<AskResult>, AgentError> {
    let node = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/ask");
    let has_parent = app
        .graph
        .relations(node)
        .map_err(for_agent)?
        .parent
        .is_some();
    if has_parent {
        let sent = match req.doc {
            Some(_) => Err(Refusal::DocHasParent),
            None => app.graph.ask_parent(node, &req.question, Some(&via)),
        };
        return recorded(
            &app,
            node,
            PARENT,
            Kind::Queue,
            MessageKind::Ask,
            &via,
            sent.map(AskResult::Parent),
        );
    }
    let asked = match unchecked_link(&app, node, &req.question) {
        Ok(Some(link)) => Err(Refusal::UncheckedDoc(link)),
        Ok(None) => attach(req.doc.as_deref()),
        Err(e) => Err(e),
    }
    .and_then(|doc| {
        let id = app.graph.ask(node, &req.question, doc.clone())?;
        let open = app.graph.open_questions(node)?;
        Ok(AskResult::Inbox(Asked {
            id,
            doc,
            open: open.into_iter().map(|q| q.id).collect(),
        }))
    });
    recorded(
        &app,
        node,
        INBOX,
        Kind::Queue,
        MessageKind::Ask,
        &via,
        asked,
    )
}

pub(super) async fn agent_answered(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<Option<AnsweredRequest>>,
) -> Result<Json<Vec<WaitId>>, AgentError> {
    let node = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/answered");
    let ids = req.unwrap_or_default().ids;
    let answered = app.graph.answered(node, &ids);
    if answered.is_ok() {
        app.turn_ends.called(node);
    }
    let to = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    traced(
        &app,
        node,
        Attempt::verb(&to),
        &via,
        answered.map_err(Problem::from),
    )
}

pub(super) async fn agent_adopt(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AdoptRequest>,
) -> Result<Json<()>, AgentError> {
    let parent = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/adopt");
    let adopted = app
        .graph
        .find(&req.name)
        .map_err(Problem::from)
        .and_then(|child| {
            let limits = app.tree_limits()?;
            Ok(app.graph.adopt(parent, child, limits)?)
        });
    traced(&app, parent, Attempt::verb(&req.name), &via, adopted)
}

pub(super) async fn agent_remove(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<RemoveRequest>,
) -> Result<Json<Vec<String>>, AgentError> {
    let caller = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/rm");
    let removed = app.tree_limits().and_then(|limits| {
        let (removed, moved) = app.graph.remove_child(caller, &req.name, limits)?;
        app.host.kill(removed);
        Ok(moved)
    });
    traced(&app, caller, Attempt::verb(&req.name), &via, removed)
}
