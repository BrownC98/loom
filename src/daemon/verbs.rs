use super::*;
use crate::protocol::{AgentId, ModelChoice};

pub(super) async fn agent_spawn(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SpawnRequest>,
) -> Result<Json<NodeId>, AgentError> {
    let parent = app.caller(&headers)?;
    spawn_traced(&app, &headers, parent, req).await
}

pub(super) async fn endpoint_spawn(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SpawnRequest>,
) -> Result<Json<NodeId>, AgentError> {
    let parent = app.caller(&headers)?;
    let agent = app
        .graph
        .agent_of(parent)
        .map_err(|e| for_agent(Refusal::from(e)))?;
    if agent
        .as_ref()
        .and_then(profile::profile_of)
        .is_some_and(|p| profile::adapter(p).spawns_at_daemon())
    {
        return Err((
            StatusCode::MISDIRECTED_REQUEST,
            "This node spawns at the daemon address, with approval to run outside its sandbox."
                .into(),
        ));
    }
    spawn_traced(&app, &headers, parent, req).await
}

async fn spawn_traced(
    app: &Arc<App>,
    headers: &HeaderMap,
    parent: NodeId,
    req: SpawnRequest,
) -> Result<Json<NodeId>, AgentError> {
    let via = gate::via(headers, "/api/agent/spawn");
    let name = req.run.name();
    let spawned = spawn(app, parent, req).await;
    traced(app, parent, Attempt::verb(&name), &via, spawned)
}

pub(super) async fn spawn(
    app: &Arc<App>,
    parent: NodeId,
    req: SpawnRequest,
) -> Result<NodeId, Problem> {
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
    let name = req.run.name();
    let (role, review, brief) = match &req.run {
        SpawnRun::Role { role, .. } => (role.trim(), None, req.brief.clone()),
        SpawnRun::Review { role, branch } => (
            role.trim(),
            Some(branch.trim()),
            format!("{}\n\n{REVIEW_RULES}", req.brief.trim())
                .trim_start()
                .to_owned(),
        ),
    };
    let start = role_start(
        &app.settings,
        role,
        &brief,
        req.agent.clone(),
        req.choice.clone(),
    )?;
    let worktree = match &req.worktree {
        Some(branch) => {
            let repo = app.repos.path(place.repo)?;
            let workspace = app.workspace(&repo).await?;
            let from = std::path::Path::new(&place.worktree);
            let unowned = |path: &str| app.graph.nodes_in(path).is_ok_and(|nodes| nodes.is_empty());
            let made = worktree::create(&repo, from, &workspace, branch, "HEAD", unowned).await?;
            Some((repo, made))
        }
        None => None,
    };
    let made = worktree.as_ref().map(|(_, made)| made.as_str());
    let cwd = std::path::Path::new(made.unwrap_or(&place.worktree));
    let child = NewChild {
        name: &name,
        worktree: made,
        agent: start.agent(),
        role,
        task: &dir.folder,
        review,
    };
    let spawned = spawn_child(app, parent, child, &start, cwd, &dir, &given).await;
    if let (Ok(_), Some(fresh)) = (&spawned, fresh) {
        fresh.keep();
    }
    if let (Ok(id), Some(_)) = (&spawned, review)
        && let Err(e) = app.graph.introduce_reviewer(*id)
    {
        eprintln!("loom: 검토자 {id}를 다른 검토자에게 알리지 못했다: {e}");
    }
    spawned
}

pub(super) fn role_refusal(e: RoleProblem) -> Refusal {
    match e {
        RoleProblem::Unknown { role, names } => Refusal::UnknownRole { role, names },
        RoleProblem::NoAgent { role } => Refusal::RoleHasNoAgent { role },
        RoleProblem::Launch(e) => Refusal::BadRoleSetting(e.to_string()),
    }
}

pub(super) async fn agent_set_role(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SetRole>,
) -> Result<Json<Roles>, AgentError> {
    let caller = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/roles/set");
    let set = set_role(&app, caller, &req);
    if set.is_ok() {
        app.graph.publish_view_changed();
    }
    traced(&app, caller, Attempt::verb(&req.role), &via, set)
}

fn set_role(app: &App, caller: NodeId, req: &SetRole) -> Result<Roles, Problem> {
    if app.graph.has_parent(caller)? {
        return Err(Refusal::RolesHaveParent.into());
    }
    use crate::settings::SetRoleProblem;
    app.settings
        .set_role(req, profile::check_known_choice)
        .map_err(|e| match e {
            SetRoleProblem::Db(e) => internal(e),
            SetRoleProblem::Shape(e) => Refusal::BadRoleSetting(e).into(),
            SetRoleProblem::Role(e) => role_refusal(e).into(),
        })
}

fn role_start(
    settings: &Settings,
    role: &str,
    brief: &str,
    agent: Option<AgentId>,
    choice: ModelChoice,
) -> Result<Start, Problem> {
    let roles = settings.get().map_err(internal)?.roles;
    let assigned = roles.assign(role, brief).map_err(role_refusal)?;
    let chosen = match (agent, choice.is_empty()) {
        (None, true) => NewAgent {
            agent: assigned.agent,
            choice: assigned.choice,
        },
        (agent, _) => relaunch::chosen_agent(
            &roles,
            Some(role),
            NewAgent {
                agent: agent.unwrap_or(assigned.agent),
                choice,
            },
        )?,
    };
    Ok(Start::Agent {
        agent: chosen.agent,
        brief: assigned.brief,
        choice: chosen.choice,
        resume: None,
    })
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
        if matches!(what, MessageKind::Ask) {
            app.turn_ends.asked(from);
        }
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

fn note(app: &Arc<App>, from: NodeId, req: &NoteRequest, via: &Via) -> Result<Sent, Refusal> {
    app.kind_word(&req.body)?;
    if req.mode != Kind::Queue {
        let (to, agent) = app.graph.agent_at(from, &req.to)?;
        let target = format!("#{}", to.0);
        if !agent.as_ref().is_some_and(profile::linked) {
            return Err(profile::not_steerable(req.to.clone()));
        }
        if req.mode == Kind::Interrupt {
            let inside = app.wsl_at(to)?;
            let mut stopped = false;
            let (sent, shells) =
                app.graph
                    .interrupt(from, &target, &req.body, Some(via), |to, shells| {
                        if inside.is_none() && !shells.is_empty() {
                            stopped = app.host.stop_guarded(to, shells);
                        }
                    })?;
            match inside {
                Some(at) => app.end_guarded_inside(to, &at, shells),
                None if stopped => app.release_guarded(to, &shells),
                None => {}
            }
            return Ok(sent);
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
    let sent = app.kind_word(&req.body).and_then(|()| {
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
            let unchecked = if !app.graph.has_parent(node)? {
                unchecked_link(&app.graph, node, &req.summary)?
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
    let word = app.kind_word(&req.summary);
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
        keep: &req.keep,
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
    let done = match done {
        Ok(unkept) => {
            let swept = app.sweep(node).await;
            let mut kept: Vec<String> = unkept
                .into_iter()
                .map(|name| {
                    format!(
                        "{name}: it is not a done child of yours, so loom stored no --keep for it."
                    )
                })
                .collect();
            kept.extend(swept.kept);
            kept.extend(swept.problems);
            Ok(Reported {
                warning: warning.clone(),
                removed: swept.removed,
                kept,
            })
        }
        Err(refused) => Err(refused),
    };
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

fn unchecked_link(graph: &Graph, node: NodeId, text: &str) -> Result<Option<String>, Refusal> {
    let folder = graph.place_of(node)?.worktree;
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
    if app.graph.has_parent(node).map_err(for_agent)? {
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
    let asked = match unchecked_link(&app.graph, node, &req.question) {
        Ok(Some(link)) => Err(Refusal::UncheckedDoc(link)),
        Ok(None) => req
            .doc
            .as_deref()
            .map_or(Ok(()), |path| user_doc::doc_label(&req.question, path))
            .and_then(|()| attach(req.doc.as_deref())),
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
            app.graph.adopt(parent, child, limits)?;
            Ok(child)
        })
        .map(|child| app.parent_changed(&[child]));
    traced(&app, parent, Attempt::verb(&req.name), &via, adopted)
}

pub(super) async fn agent_detach(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<DetachRequest>,
) -> Result<Json<()>, AgentError> {
    let caller = app.caller(&headers)?;
    let via = gate::via(&headers, "/api/agent/detach");
    let detached = app
        .graph
        .any_node(caller, &req.name)
        .and_then(|child| app.graph.detach(child).map(|()| child))
        .map(|child| app.parent_changed(&[child]))
        .map_err(Problem::from);
    traced(&app, caller, Attempt::verb(&req.name), &via, detached)
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
        let (ids, names): (Vec<NodeId>, Vec<String>) = moved.into_iter().unzip();
        app.parent_changed(&ids);
        Ok(names)
    });
    traced(&app, caller, Attempt::verb(&req.name), &via, removed)
}
