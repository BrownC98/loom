use super::*;

impl App {
    pub(super) fn tree_limits(&self) -> Result<TreeLimits, Problem> {
        Ok(self.settings.get().map_err(internal)?.tree_limits)
    }

    pub(super) async fn start_root(
        self: &Arc<Self>,
        place: &Place,
        start: &Start,
    ) -> Result<NodeId, Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let made = match start.agent() {
            Some(agent) => {
                let repo = self.repos.path(place.repo)?;
                let name = self.graph.next_root_name(Some(&agent)).map_err(internal)?;
                let tasks = self.tasks_for(&place.worktree).await?;
                Some(tasks.new_dir(&task::base(&repo, &name)).map_err(internal)?)
            }
            None => None,
        };
        let dir = made.as_ref().map(|(dir, _)| dir);
        let prepared = self.prepare(start, cwd, Position::Root, dir).await?;
        let discard = prepared.discarder();
        let id = self
            .graph
            .add_root(
                place,
                start.agent(),
                dir.map(|d| d.folder.as_str()),
                &prepared.launched(),
                |id| {
                    if let Some(dir) = dir {
                        task::create(&dir.local, "")
                            .map_err(|e| format!("{}: {e}", dir.local.display()))?;
                    }
                    self.launch(id, prepared)
                },
            )
            .inspect_err(|_| discard())?;
        if let Some((_, fresh)) = made {
            fresh.keep();
        }
        Ok(id)
    }

    pub(super) async fn tasks_for(&self, worktree: &str) -> Result<task::Tasks, Problem> {
        match wsl::of(std::path::Path::new(worktree)) {
            Some(at) => {
                let home = self.homes.get(&at.distro).await.map_err(LaunchError::Wsl)?;
                Ok(task::Tasks::wsl(&home, |linux| {
                    PathBuf::from(wsl::windows_path(&at.distro, linux))
                }))
            }
            None => task::Tasks::pc().ok_or_else(|| internal("홈 폴더를 찾지 못했다")),
        }
    }

    pub(super) async fn task_dir(&self, node: NodeId) -> Result<task::TaskDir, Problem> {
        let place = self.graph.place_of(node).map_err(internal)?;
        let tasks = self.tasks_for(&place.worktree).await?;
        let dir = match self.graph.task_of(node).map_err(internal)? {
            Some(folder) => tasks.dir(&folder),
            None => {
                let repo = self.repos.path(place.repo)?;
                let name = self.graph.relations(node).map_err(internal)?.me.name;
                let (new, fresh) = tasks.new_dir(&task::base(&repo, &name)).map_err(internal)?;
                let kept = self.graph.keep_task(node, &new.folder).map_err(internal)?;
                if kept == new.folder {
                    fresh.keep();
                }
                tasks.dir(&kept)
            }
        };
        task::create(&dir.local, "")
            .map_err(|e| internal(format!("{}: {e}", dir.local.display())))?;
        Ok(dir)
    }

    pub(super) async fn setup(self: &Arc<Self>, place: &Place) -> Result<(), Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let Some(line) = worktree::setup_script(cwd) else {
            return Ok(());
        };
        let start = Start::Command { line };
        let prepared = self.prepare(&start, cwd, Position::Root, None).await?;
        let discard = prepared.discarder();
        self.graph
            .add_root(place, start.agent(), None, &prepared.launched(), |id| {
                self.setups.lock().unwrap().insert(id);
                self.launch(id, prepared)
            })
            .inspect_err(|_| discard())?;
        Ok(())
    }

    pub(super) fn remove(&self, node: NodeId) -> rusqlite::Result<()> {
        self.graph.remove(node)?;
        self.host.kill(node);
        Ok(())
    }

    pub(super) fn launch(self: &Arc<Self>, id: NodeId, prepared: Prepared) -> Result<(), String> {
        let Prepared {
            cmd,
            setup,
            token,
            profile,
            codex,
            opencode2,
            first_input,
            account: _,
            inside,
            rules: _,
        } = prepared;
        self.first_inputs.launched(id, first_input);
        self.turn_ends.launched(id);
        let distro = inside.map(|inside| inside.distro);
        let on_exit = self.on_exit(id, distro, &token, &codex, &opencode2);
        self.host.spawn(id, cmd, setup, on_exit)?;
        self.follow_screen(id, profile, &token);
        self.follow_servers(id, codex, opencode2, &token);
        Ok(())
    }

    pub(super) async fn resume(self: &Arc<Self>) -> rusqlite::Result<()> {
        let mut distros = std::collections::HashSet::new();
        for node in self.graph.active()? {
            let (codex, opencode2) = match node.server.clone() {
                Some(ServerLink::Codex { url, pid, thread }) => {
                    match CodexServer::reconnect(url, pid, thread).await {
                        Ok(server) => (Some(Arc::new(server)), None),
                        Err(e) => {
                            eprintln!(
                                "loom: 노드 {}의 app-server에 다시 붙지 못했다: {e}",
                                node.id.0
                            );
                            (None, None)
                        }
                    }
                }
                Some(ServerLink::OpenCode2 {
                    url,
                    pid,
                    session,
                    password,
                }) => (
                    None,
                    Some(Arc::new(opencode2::Node::reconnect(
                        url, pid, session, password,
                    ))),
                ),
                None => (None, None),
            };
            let profile = node.agent.as_ref().and_then(profile::profile_of);
            let distro = self
                .node_wsl(Some(node.id))
                .ok()
                .flatten()
                .map(|at| at.distro);
            let on_exit = self.on_exit(node.id, distro.clone(), &node.token, &codex, &opencode2);
            if let Err(unattached) = self.host.resume(node.id, on_exit) {
                if let Some(server) = &node.server {
                    stop_server(server);
                }
                self.graph.lost(node.id)?;
                if unattached == node_host::Unattached::Gone
                    && let Some(distro) = distro
                {
                    end_leftovers(
                        &tokio::runtime::Handle::current(),
                        node.id,
                        distro,
                        node.token,
                    );
                }
                continue;
            }
            self.follow_screen(node.id, profile, &node.token);
            self.follow_servers(node.id, codex, opencode2, &node.token);
            distros.extend(distro);
        }
        for distro in distros {
            let app = self.clone();
            tokio::spawn(async move { app.open_tunnel(&distro).await });
        }
        self.host.end_others();
        self.logins.end_others();
        Ok(())
    }

    fn on_exit(
        self: &Arc<Self>,
        id: NodeId,
        distro: Option<String>,
        token: &str,
        codex: &Option<Arc<CodexServer>>,
        opencode2: &Option<Arc<opencode2::Node>>,
    ) -> impl FnOnce(Exit) + use<> {
        let app = self.clone();
        let discard = discarder(codex, opencode2);
        let runtime = tokio::runtime::Handle::current();
        let token = token.to_owned();
        move |exit| {
            let _recheck = tunnels::Recheck::after(&app, distro.clone());
            app.codex.lock().unwrap().remove(&id);
            app.opencode2.lock().unwrap().remove(&id);
            discard();
            if exit != Exit::Lost
                && let Some(distro) = distro
            {
                end_leftovers(&runtime, id, distro, token);
            }
            if app.relaunches.exited(id) {
                return;
            }
            app.sessions.exited(id);
            let setup = app.setups.lock().unwrap().remove(&id);
            if exit == Exit::Command(0)
                && (setup || !app.host.has_output(id))
                && app.graph.discard_finished_shell(id).unwrap_or(false)
            {
                app.host.kill(id);
                return;
            }
            let slept = matches!(exit, Exit::Command(_))
                && app.sleeps.stopping(id)
                && app.graph.slept(id).unwrap_or(false);
            if slept && let Err(e) = app.host.keep(id) {
                eprintln!("loom: 잠든 노드 {}의 화면을 남기지 못했다: {e}", id.0);
            }
            let _ = match exit {
                _ if slept => Ok(()),
                Exit::Command(code) => app.graph.exited(id, code),
                Exit::Setup(code) => app.graph.setup_failed(id, code, &app.host.tail(id)),
                Exit::Lost => app.graph.lost(id),
            };
            app.process_ended(id);
            app.sleeps.exited(id);
            app.graph.wake_mail();
            app.idle.end(id, IdleEnd::Exit);
        }
    }

    fn follow_screen(self: &Arc<Self>, id: NodeId, profile: Option<Profile>, token: &str) {
        let Some(attached) = self.host.attach(id) else {
            return;
        };
        if !profile::reads_screen(profile) {
            return;
        }
        let mut started = self.sessions.launched(id, token);
        let (app, token) = (self.clone(), token.to_owned());
        tokio::spawn(async move {
            let mut output = attached.output;
            let mut changed = true;
            loop {
                if !changed {
                    tokio::select! {
                        received = output.recv() => match received {
                            Ok(_) | Err(RecvError::Lagged(_)) => {}
                            Err(RecvError::Closed) => return,
                        },
                        sent = started.changed() => if sent.is_err() {
                            return;
                        },
                    }
                }
                loop {
                    match tokio::time::timeout(SCREEN_QUIET, output.recv()).await {
                        Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
                        Ok(Err(RecvError::Closed)) => return,
                        Err(_) => break,
                    }
                }
                changed = false;
                let now = app.host.screen(id);
                let found = profile::wait(profile, &now, *started.borrow_and_update());
                let wait = found.as_ref().map(|(kind, text)| (*kind, text.as_str()));
                if let Err(e) = app.graph.set_wait(&token, WaitSource::Screen, wait) {
                    eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", id.0);
                }
                let prompt = profile::prompt(profile, &now);
                if let Err(e) = app.graph.set_prompt(id, prompt) {
                    eprintln!("loom: 노드 {}의 승인 창을 쓰지 못했다: {e}", id.0);
                }
            }
        });
    }

    fn follow_servers(
        self: &Arc<Self>,
        id: NodeId,
        codex: Option<Arc<CodexServer>>,
        opencode2: Option<Arc<opencode2::Node>>,
        token: &str,
    ) {
        if let Some(server) = codex {
            self.codex.lock().unwrap().insert(id, server.clone());
            let (app, mut limited) = (self.clone(), server.limited());
            tokio::spawn(async move {
                while limited.changed().await.is_ok() {
                    app.rate_limited(id, crate::protocol::Provider::Codex);
                }
            });
            tokio::spawn(follow_codex_model(
                self.graph.clone(),
                id,
                token.to_owned(),
                server.clone(),
            ));
            let (app, mut thread, token) = (self.clone(), server.thread(), token.to_owned());
            tokio::spawn(async move {
                if thread.wait_for(Option::is_some).await.is_ok() {
                    app.sessions.started(id, &token);
                }
            });
            let current = Arc::downgrade(&server);
            tokio::spawn(follow_server_turns(
                self.clone(),
                id,
                server.thread(),
                |thread| thread.as_ref().map(|t| t.status == Status::Busy),
                move |app: &App| {
                    let now = app.codex.lock().unwrap().get(&id).map(Arc::downgrade);
                    now.is_some_and(|now| now.ptr_eq(&current))
                },
            ));
            tokio::spawn(deliver_to_codex(self.clone(), id, server));
        }
        if let Some(node) = opencode2 {
            self.opencode2.lock().unwrap().insert(id, node.clone());
            tokio::spawn(follow_model(self.graph.clone(), id, node.model()));
            let current = Arc::downgrade(&node);
            tokio::spawn(follow_server_turns(
                self.clone(),
                id,
                node.working(),
                |working| Some(*working),
                move |app: &App| {
                    let now = app.opencode2.lock().unwrap().get(&id).map(Arc::downgrade);
                    now.is_some_and(|now| now.ptr_eq(&current))
                },
            ));
            tokio::spawn(follow_permission(
                self.graph.clone(),
                id,
                token.to_owned(),
                node.permission(),
            ));
            let mailbox = NodeMailbox {
                app: self.clone(),
                node: id,
            };
            tokio::spawn(async move { node.run(mailbox).await });
        }
    }

    pub(super) fn caller(&self, headers: &HeaderMap) -> Result<NodeId, AgentError> {
        let token = bearer(headers)?;
        let node = self
            .graph
            .caller(token)
            .map_err(|e| for_agent(Refusal::from(e)))?
            .ok_or_else(unauthorized)?;
        self.sessions.started(node, token);
        Ok(node)
    }

    pub(super) async fn prepare(
        &self,
        start: &Start,
        cwd: &std::path::Path,
        position: Position,
        task: Option<&task::TaskDir>,
    ) -> Result<Prepared, Problem> {
        let profile = start.profile();
        let provider = profile.and_then(Profile::provider);
        let settings = self.settings.get().map_err(internal)?;
        let launch = Launch {
            position,
            skills: settings.always_on,
        };
        let token = new_token().map_err(internal)?;
        let mut env = self.node_env(&token)?.to_vec();
        let plugin = self.plugin.dir(launch.skills);
        let site = match wsl::of(cwd) {
            Some(at) => {
                self.open_tunnel(&at.distro).await;
                Site::wsl(
                    wsl::inside(&at, &profile::commands(start), &self.shim, &[&plugin])
                        .await
                        .map_err(LaunchError::Wsl)?,
                )
            }
            None => self.pc_site(),
        };
        if profile == Some(Profile::Cursor)
            && let Some(hooks) = user_cursor_hooks(&site)
            && let Err(e) = cursor::open_gate(&hooks)
        {
            eprintln!(
                "loom: {}에 쉬는 대기 훅을 넣지 못했다: {e}",
                hooks.display()
            );
        }
        let account = provider.and_then(|p| settings.account(p, site.distro()));
        if let Some(dir) = task {
            env.push((task::ENV, dir.shown.clone().into()));
        }
        if let Some(provider) = provider {
            env.extend(site.env(provider, account));
        }
        let inside = site.inside();
        let (plugin, skills) = match inside.as_ref().and_then(|i| i.path(0)) {
            Some(inside) => (
                PathBuf::from(inside),
                PathBuf::from(format!("{inside}/skills")),
            ),
            None => (plugin.clone(), self.plugin.skills(launch.skills)),
        };
        if profile == Some(Profile::Omp) {
            omp::write_config(
                &self.plugin.dir(launch.skills),
                &omp::config(&plugin, launch),
            )
            .map_err(internal)?;
        }
        let codex = match profile {
            Some(Profile::Codex) => {
                let config: Vec<String> = self
                    .codex_config
                    .iter()
                    .cloned()
                    .chain(codex::server_args(launch))
                    .collect();
                let server = CodexServer::start(cwd, inside, &env, &config)
                    .await
                    .map_err(|cause| LaunchError::Server {
                        program: "codex app-server".into(),
                        cause,
                    })?;
                let call = |method, params| server.call(method, params);
                if let Err(e) = codex::add_skills(call, &skills).await {
                    eprintln!("loom: Codex 서버에 내장 스킬을 더하지 못했다: {e}");
                }
                Some(Arc::new(server))
            }
            _ => None,
        };
        let opencode2 = match profile {
            Some(Profile::OpenCode2) => {
                let password = new_token().map_err(internal)?;
                let resume = match start {
                    Start::Agent { resume, .. } => resume.as_deref(),
                    Start::Command { .. } => None,
                };
                let node =
                    opencode2::Node::start(cwd, inside, &env, &skills, password, launch, resume)
                        .await
                        .map_err(|cause| LaunchError::Server {
                            program: format!("{} serve", opencode2::CMD),
                            cause,
                        })?;
                Some(Arc::new(node))
            }
            _ => None,
        };
        let remote = match (&codex, &opencode2) {
            (Some(server), _) => Some(Remote::Codex { url: server.url() }),
            (_, Some(node)) => Some(node.remote()),
            _ => None,
        };
        let discard = discarder(&codex, &opencode2);
        let mut cmd = profile::command(
            start,
            cwd,
            inside,
            remote,
            &settings.bypass_off,
            &plugin,
            launch,
        )
        .inspect_err(|_| discard())?;
        for (key, value) in &env {
            cmd.env(key, value);
        }
        let inside = site.into_inside();
        if let Some(inside) = &inside {
            cmd = inside.wrap(&cmd);
        }
        Ok(Prepared {
            cmd,
            inside,
            setup: None,
            token,
            account: account.cloned(),
            rules: profile.map(|p| profile::start_digests(p, launch)),
            profile,
            codex,
            opencode2,
            first_input: match start {
                Start::Agent { brief, .. } => !brief.trim().is_empty(),
                Start::Command { .. } => false,
            },
        })
    }

    pub(super) fn node_wsl(&self, node: Option<NodeId>) -> Result<Option<wsl::WslPath>, Problem> {
        let Some(node) = node else { return Ok(None) };
        let place = self.graph.place_of(node).map_err(internal)?;
        Ok(wsl::of(std::path::Path::new(&place.worktree)))
    }

    pub(super) fn end_guarded_inside(&self, node: NodeId) {
        let Some(at) = self.node_wsl(Some(node)).ok().flatten() else {
            return;
        };
        let token = self
            .graph
            .active()
            .ok()
            .and_then(|nodes| nodes.into_iter().find(|n| n.id == node))
            .map(|n| n.token);
        let Some(token) = token else { return };
        tokio::spawn(async move {
            if let Err(e) = wsl::end_shells(&at.distro, &token).await {
                eprintln!("loom: 노드 {}의 명령을 멈추지 못했다: {e}", node.0);
            }
        });
    }

    fn node_env(&self, token: &str) -> Result<[(&'static str, OsString); 4], Problem> {
        Ok([
            ("LOOM_URL", self.url.clone().into()),
            ("LOOM_SPAWN_URL", self.spawn_url.clone().into()),
            ("LOOM_TOKEN", token.into()),
            ("PATH", node_path().map_err(internal)?),
        ])
    }
}

pub(super) struct Prepared {
    pub(super) cmd: portable_pty::CommandBuilder,
    pub(super) inside: Option<wsl::Inside>,
    pub(super) setup: Option<portable_pty::CommandBuilder>,
    pub(super) token: String,
    pub(super) account: Option<AccountId>,
    pub(super) rules: Option<crate::protocol::StartDigests>,
    pub(super) profile: Option<Profile>,
    pub(super) codex: Option<Arc<CodexServer>>,
    pub(super) opencode2: Option<Arc<opencode2::Node>>,
    pub(super) first_input: bool,
}

impl Prepared {
    pub(super) fn launched(&self) -> Launched {
        Launched {
            token: self.token.clone(),
            server: match (&self.codex, &self.opencode2) {
                (Some(server), _) => Some(server.link()),
                (_, Some(node)) => Some(node.link()),
                _ => None,
            },
            account: self.account.clone(),
            rules: self.rules.clone(),
        }
    }

    pub(super) fn shell(&self, line: &str) -> portable_pty::CommandBuilder {
        match &self.inside {
            Some(inside) => inside.wrap(&portable_pty::CommandBuilder::from_argv(wsl::shell(line))),
            None => profile::shell(line),
        }
    }

    pub(super) fn discarder(&self) -> impl Fn() + use<> {
        discarder(&self.codex, &self.opencode2)
    }
}

fn user_cursor_hooks(site: &Site) -> Option<PathBuf> {
    match site.inside() {
        Some(inside) => Some(inside.local(&format!("{}/{}", inside.home(), cursor::USER_HOOKS))),
        None if cfg!(windows) => None,
        None => std::env::home_dir().map(|home| home.join(cursor::USER_HOOKS)),
    }
}

fn end_leftovers(runtime: &tokio::runtime::Handle, id: NodeId, distro: String, token: String) {
    runtime.spawn(async move {
        if let Err(e) = wsl::end_token(&distro, &token).await {
            eprintln!("loom: 노드 {}가 남긴 프로세스를 끝내지 못했다: {e}", id.0);
        }
    });
}

fn discarder(
    codex: &Option<Arc<CodexServer>>,
    opencode2: &Option<Arc<opencode2::Node>>,
) -> impl Fn() + use<> {
    let (codex, opencode2) = (codex.clone(), opencode2.clone());
    move || {
        if let Some(server) = &codex {
            server.stop();
        }
        if let Some(node) = &opencode2 {
            node.stop();
        }
    }
}

struct NodeMailbox {
    app: Arc<App>,
    node: NodeId,
}

impl Mailbox for NodeMailbox {
    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let mut changed = Box::pin(self.app.graph.mail_changed());
        changed.as_mut().enable();
        changed
    }

    fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String> {
        self.app
            .take_mail(self.node, kinds, limit_bytes)
            .map_err(|e| e.to_string())
    }

    fn ack(&self, taken: &[Delivery]) -> Result<(), String> {
        let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
        self.app
            .graph
            .ack(self.node, &ids)
            .map_err(|e| e.to_string())
    }

    fn release(&self, taken: &[Delivery]) -> Result<(), String> {
        let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
        self.app
            .graph
            .release(self.node, &ids)
            .map_err(|e| e.to_string())
    }

    fn turn_ended(&self, reply: Option<String>) -> Option<String> {
        if let Some(text) = reply {
            self.app.turn_ends.said(self.node, text);
        }
        self.app.turn_end(self.node).map(str::to_owned)
    }

    fn human_input(&self) -> Option<String> {
        self.app.human_input(self.node, None).ok().flatten()
    }
}

async fn follow_model(graph: Arc<Graph>, node: NodeId, mut model: watch::Receiver<Model>) {
    while model.changed().await.is_ok() {
        let now = model.borrow_and_update().clone();
        if let Err(e) = graph.set_model(node, &now) {
            eprintln!("loom: 노드 {}의 모델을 쓰지 못했다: {e}", node.0);
        }
    }
}

pub(super) async fn follow_server_turns<T>(
    app: Arc<App>,
    node: NodeId,
    mut status: watch::Receiver<T>,
    busy: fn(&T) -> Option<bool>,
    current: impl Fn(&App) -> bool,
) {
    let mut fed = None;
    loop {
        let now = busy(&status.borrow_and_update());
        if let Some(busy) = now
            && now != fed
            && current(&app)
        {
            match busy {
                true => app.feed(node, AgentEvent::ServerBusy),
                false => {
                    app.feed(node, AgentEvent::ServerIdle);
                    app.turn_ended(node);
                }
            }
            fed = now;
        }
        if status.changed().await.is_err() {
            if current(&app) {
                app.feed(node, AgentEvent::ServerLost);
            }
            return;
        }
    }
}

async fn follow_permission(
    graph: Arc<Graph>,
    node: NodeId,
    token: String,
    mut asked: watch::Receiver<Option<opencode2::Permission>>,
) {
    let show = |now: Option<&opencode2::Permission>| {
        if let Err(e) = graph.set_prompt(node, now.map(|p| p.prompt())) {
            eprintln!("loom: 노드 {}의 승인 창을 쓰지 못했다: {e}", node.0);
        }
        let summary = now.map(|p| p.summary());
        let wait = summary.as_deref().map(|s| (WaitKind::Approval, s));
        if let Err(e) = graph.set_wait(&token, WaitSource::OpenCode2, wait) {
            eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", node.0);
        }
    };
    let mut shown: Option<opencode2::Permission> = None;
    loop {
        let now = asked.borrow_and_update().clone();
        if shown.is_some() && now.is_some() && shown != now {
            show(None);
        }
        show(now.as_ref());
        shown = now;
        if asked.changed().await.is_err() {
            break;
        }
    }
    show(None);
}

pub(super) fn bearer(headers: &HeaderMap) -> Result<&str, AgentError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)
}

fn unauthorized() -> AgentError {
    (
        StatusCode::UNAUTHORIZED,
        "Run this only inside a loom node.".into(),
    )
}

async fn follow_codex_model(
    graph: Arc<Graph>,
    node: NodeId,
    token: String,
    server: Arc<CodexServer>,
) {
    let mut thread = server.thread();
    let mut kept = thread.borrow().as_ref().map(|t| t.id.clone());
    while thread.changed().await.is_ok() {
        let Some((id, waiting)) = thread
            .borrow_and_update()
            .as_ref()
            .map(|t| (t.id.clone(), t.waiting))
        else {
            continue;
        };
        if let Err(e) = graph.set_wait(&token, WaitSource::Codex, waiting.map(|kind| (kind, ""))) {
            eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", node.0);
        }
        if kept.as_ref() != Some(&id) {
            if let Err(e) = graph.set_server(node, &server.link()) {
                eprintln!("loom: 노드 {}의 스레드를 쓰지 못했다: {e}", node.0);
            }
            kept = Some(id.clone());
        }
        let call = |method, params| server.call(method, params);
        let saved = match codex::model(call, &id).await {
            Ok(model) => graph.set_model(node, &model).map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        if let Err(e) = saved {
            eprintln!("loom: 노드 {}의 모델을 읽지 못했다: {e}", node.0);
        }
    }
}

async fn deliver_to_codex(app: Arc<App>, node: NodeId, server: Arc<CodexServer>) {
    let mut thread = server.thread();
    let mut started = false;
    let mut busy = false;
    let mut ours = false;
    loop {
        let mut mail = pin!(app.graph.mail_changed());
        mail.as_mut().enable();
        let now = thread.borrow_and_update().clone();
        if let Some(now) = now {
            let was_busy = std::mem::replace(&mut busy, now.status == Status::Busy);
            if !was_busy && busy && !std::mem::take(&mut ours) {
                codex_human_input(&app, node, &server, &now.id).await;
            }
            if was_busy && !busy && !started && codex_turn_end(&app, node, &server, &now.id).await {
                started = true;
                ours = true;
                continue;
            }
            let status = if started { Status::Busy } else { now.status };
            match app.take_mail(node, codex::kinds(status), codex::INPUT_LIMIT_BYTES) {
                Ok(taken) if !taken.is_empty() => {
                    if taken.iter().any(|d| d.kind == Kind::Interrupt) {
                        server.stop_shells();
                    }
                    let call = |method, params| server.call(method, params);
                    let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
                    match codex::deliver(call, &now.id, status, &taken).await {
                        Ok(()) => {
                            started = true;
                            ours |= status == Status::Idle;
                            if let Err(e) = app.graph.ack(node, &ids) {
                                eprintln!("loom: 노드 {}의 받음 확인을 쓰지 못했다: {e}", node.0);
                            }
                            continue;
                        }
                        Err(e) => {
                            eprintln!("loom: 노드 {}에 메시지를 넘기지 못했다: {e}", node.0);
                            if let Err(e) = app.graph.release(node, &ids) {
                                eprintln!(
                                    "loom: 노드 {}의 메시지를 큐로 돌리지 못했다: {e}",
                                    node.0
                                );
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("loom: 노드 {}의 메일함을 읽지 못했다: {e}", node.0),
            }
        }
        tokio::select! {
            _ = mail => {}
            changed = thread.changed() => {
                if changed.is_err() {
                    return;
                }
                started = false;
            }
        }
    }
}

impl App {
    pub(super) fn human_input(
        &self,
        node: NodeId,
        prompt: Option<&str>,
    ) -> Result<Option<String>, Refusal> {
        self.graph.human_input(node)?;
        self.turn_ends.human(node);
        if let Some(text) = prompt
            && !self.first_inputs.injected(node)
        {
            self.graph.human_typed(node, text)?;
        }
        Ok(profile::questions_note(&self.graph.open_questions(node)?))
    }
}

async fn codex_human_input(app: &App, node: NodeId, server: &CodexServer, thread: &str) {
    let text = match app.human_input(node, None) {
        Ok(Some(text)) => text,
        Ok(None) => return,
        Err(e) => return eprintln!("loom: 노드 {}의 사람 입력을 처리하지 못했다: {e}", node.0),
    };
    let call = |method, params| server.call(method, params);
    if let Err(e) = codex::steer(call, thread, &text).await {
        eprintln!("loom: 노드 {}에 열린 질문을 알리지 못했다: {e}", node.0);
    }
}

async fn codex_turn_end(app: &App, node: NodeId, server: &CodexServer, thread: &str) -> bool {
    let call = |method, params| server.call(method, params);
    match codex::last_reply(call, thread).await {
        Ok(Some(text)) => app.turn_ends.said(node, text),
        Ok(None) => {}
        Err(e) => eprintln!("loom: 노드 {}의 답 글을 읽지 못했다: {e}", node.0),
    }
    let Some(nudge) = app.turn_end(node) else {
        return false;
    };
    match codex::start_turn(call, thread, nudge).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("loom: 노드 {}를 다시 일하게 하지 못했다: {e}", node.0);
            false
        }
    }
}

fn stop_server(server: &ServerLink) {
    let (ServerLink::Codex { pid, .. } | ServerLink::OpenCode2 { pid, .. }) = server;
    if let Some(root) = pid {
        process_tree::Snapshot::now().kill_trees([*root]);
    }
}

fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn each_permission_request_gets_its_own_wait() {
        let dir = tempfile::tempdir().unwrap();
        let db = db::open(&dir.path().join("loom.db")).unwrap();
        db.lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let graph = Arc::new(Graph::open(db, crate::profile::linked).unwrap());
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
                Some(crate::protocol::AgentId("opencode2".into())),
                None,
                &launched,
                |_| Ok(()),
            )
            .unwrap();
        let request = |id: &str| -> opencode2::Permission {
            serde_json::from_value(
                serde_json::json!({ "id": id, "action": "shell", "resources": ["echo hi"] }),
            )
            .unwrap()
        };
        let waits = || {
            let views = graph.subscribe().unwrap().0;
            let view = views.into_iter().find(|v| v.id == node).unwrap();
            view.waits
                .iter()
                .map(|w| {
                    (
                        w.id,
                        w.text.clone(),
                        w.prompt.as_ref().map(|p| p.rows[0].clone()),
                    )
                })
                .collect::<Vec<_>>()
        };
        let until = |request: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let now = waits();
                if now.first().is_some_and(|w| w.2.as_deref() == Some(request)) {
                    return now;
                }
                assert!(std::time::Instant::now() < deadline, "{now:?}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let (tx, rx) = watch::channel(None);
        let follower = tokio::spawn(follow_permission(graph.clone(), node, "t".into(), rx));

        tx.send_replace(Some(request("per_1")));
        let first = tokio::task::block_in_place(|| until("per_1"));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1, "shell: echo hi");

        tx.send_replace(Some(request("per_2")));
        let second = tokio::task::block_in_place(|| until("per_2"));
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].0, first[0].0);
        assert_eq!(graph.dismiss(first[0].0), Err(Refusal::NoWait(first[0].0)));

        drop(tx);
        follower.await.unwrap();
        assert_eq!(waits(), vec![]);
    }
}
