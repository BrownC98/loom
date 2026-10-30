use super::agent::AgentPort;
use super::*;
use crate::distro::Distro;
use crate::graph::{Active, Entry};
use crate::process_tree::Identity;
use crate::profile::adapter::{LaunchAt, Record, Started};
use crate::protocol::{NODE_CALLS, NODE_CALLS_HEADER};
use crate::tunnel::ENDPOINT_ENV;

const LINUX_LOOM_MISSING: &str = "loom-linux is not next to loom.exe";

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
                let distro = self.distro(&at.distro).map_err(LaunchError::Wsl)?;
                let home = wsl::home(&distro).await.map_err(LaunchError::Wsl)?;
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
                let name = self.graph.name(node).map_err(internal)?;
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
            .add_setup(place, &prepared.launched(), |id| self.launch(id, prepared))
            .inspect_err(|_| discard())?;
        Ok(())
    }

    pub(super) fn remove(self: &Arc<Self>, node: NodeId) -> Result<Vec<NodeId>, Refusal> {
        let children = self.graph.remove(node)?;
        self.host.kill(node);
        self.parent_changed(&children);
        Ok(children)
    }

    pub(super) fn launch(self: &Arc<Self>, id: NodeId, prepared: Prepared) -> Result<(), String> {
        let Prepared {
            cmd,
            setup,
            token,
            profile,
            port,
            started: _,
            first_input,
            account: _,
            inside,
            rules: _,
            position,
        } = prepared;
        match first_input {
            true => self.first_inputs.mark(id),
            false => self.first_inputs.clear(id),
        }
        self.turn_ends.launched(id);
        let distro = inside.as_ref().map(|inside| inside.distro.clone());
        let on_exit = self.on_exit(id, distro, &token, port.clone());
        let at = match (&inside, &self.linux_loom) {
            (Some(inside), Some(loom)) => {
                let name = loom.file_name().unwrap_or_default().to_string_lossy();
                let start = inside.start(inside.bin_file(&name), &cmd, setup.as_ref());
                self.host.start_in(inside.distro.name(), start)?
            }
            (Some(_), None) => return Err(LINUX_LOOM_MISSING.into()),
            (None, _) => self.host.start(&cmd, setup.as_ref())?,
        };
        if let Err(e) = self.graph.set_host(id, &at.to_string()) {
            self.host.end_at(&at);
            return Err(e.to_string());
        }
        match self.host.resume(id, &at, on_exit) {
            Ok(()) => {
                self.follow_screen(id, profile, &token);
                port.bind(id);
                self.ptys_changed.notify_waiters();
            }
            Err(node_host::Unattached::Unreached(cause)) => {
                self.host_unreached(id, &cause).map_err(|e| e.to_string())?;
            }
            Err(e) => {
                self.host.end_at(&at);
                return Err(e.to_string());
            }
        }
        if self.position_of(id).is_ok_and(|now| now != position) {
            if let Err(e) = self.graph.plan_resume(id) {
                eprintln!("loom: 노드 {}의 다시 띄우기를 계획하지 못했다: {e}", id.0);
            }
            self.run_unless_busy(id);
        }
        Ok(())
    }

    pub(super) fn position_of(&self, id: NodeId) -> Result<Position, Refusal> {
        Ok(match self.graph.has_parent(id)? {
            true => Position::Child,
            false => Position::Root,
        })
    }

    pub(super) async fn resume(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            self.resume_node(node).await?;
        }
        Ok(())
    }

    pub(super) async fn resume_node(self: &Arc<Self>, node: Active) -> rusqlite::Result<()> {
        if !self.graph.active()?.iter().any(|n| n.id == node.id) {
            return Ok(());
        }
        let place_distro = self.place_distro(node.id);
        let profile = node.agent.as_ref().and_then(profile::profile_of);
        let distro = place_distro
            .as_deref()
            .and_then(|name| self.distro(name).ok());
        let port = AgentPort::new(self.clone(), node.token.clone());
        let on_exit = self.on_exit(node.id, distro.clone(), &node.token, port.clone());
        let app = self.clone();
        let id = node.id;
        let address = self.graph.host_of(id)?;
        self.graph.host_reached(id)?;
        let attached = tokio::task::spawn_blocking(move || {
            let at = address
                .ok_or_else(|| "no pty-host address".to_owned())
                .and_then(|text| node_host::Address::parse(&text, place_distro))
                .map_err(node_host::Unattached::Failed)?;
            app.host.resume(id, &at, on_exit)
        })
        .await
        .unwrap_or_else(|e| Err(node_host::Unattached::Failed(e.to_string())));
        if let Err(unattached) = attached {
            self.sleeps.exited(node.id);
            if let node_host::Unattached::Unreached(cause) = &unattached {
                return self.host_unreached(node.id, cause);
            }
            port.end();
            self.attach(&node, profile, port).await;
            let gone = unattached == node_host::Unattached::Gone;
            match self.graph.slept(node.id)? || self.graph.stop_lost(node.id)? {
                true => {}
                false if gone => self.graph.host_gone(node.id)?,
                false => self.graph.lost(node.id)?,
            }
            self.sweep_above(node.id);
            self.sweep_later(node.id);
            if gone && let Some(distro) = distro {
                end_leftovers(
                    &tokio::runtime::Handle::current(),
                    node.id,
                    distro,
                    node.token,
                );
            }
            return Ok(());
        }
        if !self.graph.active()?.iter().any(|n| n.id == node.id) {
            self.sleeps.exited(node.id);
            self.host.kill(node.id);
            port.end();
            self.attach(&node, profile, port).await;
            return Ok(());
        }
        self.follow_screen(node.id, profile, &node.token);
        port.bind(node.id);
        self.attach(&node, profile, port).await;
        self.ptys_changed.notify_waiters();
        if self.graph.sleeping(node.id)? {
            self.sleep_again_after_start(node);
        }
        Ok(())
    }

    async fn attach(&self, node: &Active, profile: Option<Profile>, port: Arc<AgentPort>) {
        let Some(profile) = profile else { return };
        let record = self.record_of(node);
        if let Err(e) = profile::adapter(profile).attach(&record, port).await {
            eprintln!(
                "loom: 노드 {}의 어댑터를 다시 붙이지 못했다: {e}",
                node.id.0
            );
        }
    }

    pub(super) fn record_of(&self, node: &Active) -> Record {
        let distro = self.place_distro(node.id).map(|name| self.distro(&name));
        let (state, distro) = match distro {
            None => (node.adapter.clone(), None),
            Some(Ok(distro)) => (node.adapter.clone(), Some(distro)),
            Some(Err(_)) => (None, None),
        };
        Record {
            state,
            session: node.session.clone(),
            account: node.account.clone(),
            distro,
        }
    }

    fn host_unreached(&self, node: NodeId, cause: &str) -> rusqlite::Result<()> {
        self.graph.host_unreached(node, cause)?;
        let error = format!("left unattached: {cause}");
        self.graph
            .record_about(node, |node| Entry::EndpointFailed { node, error })
    }

    pub(super) fn adopt_address_files(&self, dir: &std::path::Path) -> rusqlite::Result<()> {
        let active: Vec<NodeId> = self.graph.active()?.iter().map(|n| n.id).collect();
        for (folder, nodes) in [("nodes", true), ("logins", false)] {
            let entries = std::fs::read_dir(dir.join(folder)).into_iter().flatten();
            for entry in entries.flatten() {
                let Ok(text) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                let id = entry.file_name().to_string_lossy().parse().map(NodeId);
                match id {
                    Ok(id) if nodes && active.contains(&id) => {
                        self.graph.set_host(id, text.trim())?
                    }
                    _ => {
                        let distro = id
                            .ok()
                            .filter(|_| nodes)
                            .and_then(|id| self.place_distro(id));
                        if let Ok(at) = node_host::Address::parse(&text, distro) {
                            self.host.end_at(&at);
                        }
                    }
                }
                let _ = std::fs::remove_file(entry.path());
            }
            let _ = std::fs::remove_dir(dir.join(folder));
        }
        Ok(())
    }

    pub(super) async fn carry_links(
        self: Arc<Self>,
        mut linked: tokio::sync::mpsc::UnboundedReceiver<(NodeId, node_host::Linked)>,
        doors: tokio::sync::mpsc::UnboundedSender<(tunnel::Stream, arrival::Caller)>,
    ) {
        while let Some((node, link)) = linked.recv().await {
            let app = self.clone();
            let hear = move || {
                if let Err(e) = app.hear_lost_wait(node) {
                    eprintln!("loom: 노드 {}의 쉬는 대기를 보지 못했다: {e}", node.0);
                }
            };
            let failed = match link {
                node_host::Linked::Terminal => {
                    hear();
                    None
                }
                node_host::Linked::Streams { link, held } => {
                    let callers = arrival::callers(node, held.unwrap_or(0), hear);
                    tunnel::carry(link, callers, doors.clone())
                        .err()
                        .map(|e| format!("the daemon side of the link: {e}"))
                }
                node_host::Linked::EndpointFailed(error) => Some(error),
                node_host::Linked::Unreached(cause) => {
                    if let Err(e) = self.host_unreached(node, &cause) {
                        eprintln!("loom: 노드 {}의 붙지 않음을 적지 못했다: {e}", node.0);
                    }
                    None
                }
            };
            if let Some(error) = failed
                && let Err(e) = self
                    .graph
                    .record_about(node, |node| Entry::EndpointFailed { node, error })
            {
                eprintln!("loom: 노드 {}의 접속점 실패를 적지 못했다: {e}", node.0);
            }
        }
    }

    fn on_exit(
        self: &Arc<Self>,
        id: NodeId,
        distro: Option<Distro>,
        token: &str,
        port: Arc<AgentPort>,
    ) -> impl FnOnce(Exit) + use<> {
        let app = self.clone();
        let runtime = tokio::runtime::Handle::current();
        let token = token.to_owned();
        move |exit| {
            let _entered = runtime.enter();
            app.wakes.forget(id);
            if exit != Exit::Lost
                && let Some(distro) = distro
            {
                end_leftovers(&runtime, id, distro, token);
            }
            if app.graph.stop_exited(id).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 다시 띄우기 계획을 읽지 못했다: {e}", id.0);
                false
            }) {
                port.end();
                app.relaunch_exits.notify_waiters();
                return;
            }
            app.sessions.exited(id);
            if exit == Exit::Command(0)
                && (app.graph.is_setup(id).unwrap_or(false) || !app.host.has_output(id))
                && app.graph.discard_finished_shell(id).unwrap_or(false)
            {
                port.end();
                app.host.kill(id);
                return;
            }
            let slept = matches!(exit, Exit::Command(_) | Exit::Gone)
                && app.graph.slept(id).unwrap_or(false);
            if slept && let Err(e) = app.host.keep(id) {
                eprintln!("loom: 잠든 노드 {}의 화면을 남기지 못했다: {e}", id.0);
            }
            let _ = match exit {
                _ if slept => Ok(()),
                Exit::Command(code) => app.graph.exited(id, code),
                Exit::Setup(code) => app.graph.setup_failed(id, code, &app.host.tail(id)),
                Exit::Lost => app.graph.lost(id),
                Exit::Gone => app.graph.host_gone(id),
            };
            app.process_ended(id);
            app.sleeps.exited(id);
            app.graph.wake_mail();
            app.idle.end(id, IdleEnd::Exit);
            port.end();
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
                let prompt = profile::prompt(profile, &now);
                let written = match &found {
                    Some((kind, text)) => {
                        app.graph
                            .set_wait(&token, WaitSource::Screen, Some((*kind, text, prompt)))
                    }
                    None => app
                        .graph
                        .set_wait(&token, WaitSource::Screen, None)
                        .and_then(|()| app.graph.screen_prompt(&token, prompt)),
                };
                if let Err(e) = written {
                    eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", id.0);
                }
                if profile::input_seen(profile, &now).is_some_and(|seen| seen.empty) {
                    app.draft_ended(id);
                }
            }
        });
    }

    pub(super) fn caller(self: &Arc<Self>, headers: &HeaderMap) -> Result<NodeId, AgentError> {
        let token = bearer(headers)?;
        let node = self
            .graph
            .caller(token)
            .map_err(|e| for_agent(Refusal::from(e)))?
            .ok_or_else(unauthorized)?;
        if let Some(spoken) = unserved_protocol(headers) {
            let app = self.clone();
            tokio::spawn(async move {
                if let Err(e) = app.relaunch_unserved(node).await {
                    eprintln!(
                        "loom: 판이 맞지 않는 노드 {}를 다시 띄우지 못했다: {e:?}",
                        node.0
                    );
                }
            });
            return Err((
                StatusCode::UPGRADE_REQUIRED,
                format!(
                    "This loom command speaks node protocol {spoken}, but the daemon speaks {}-{}. loom relaunches this node with the current loom. End your turn.",
                    NODE_CALLS.oldest, NODE_CALLS.now
                ),
            ));
        }
        self.sessions.started(node, token);
        Ok(node)
    }

    pub(super) async fn prepare(
        self: &Arc<Self>,
        start: &Start,
        cwd: &std::path::Path,
        position: Position,
        task: Option<&task::TaskDir>,
    ) -> Result<Prepared, Problem> {
        let profile = start.profile();
        let accounted = profile.and_then(Profile::accounts);
        let settings = self.settings.get().map_err(internal)?;
        let launch = Launch {
            position,
            skills: settings.always_on,
        };
        let token = new_token().map_err(internal)?;
        let mut env = self.node_env(&token)?.to_vec();
        let plugin_here = self.plugin.dir(launch.skills);
        let site = match wsl::of(cwd) {
            Some(at) => {
                let distro = self.distro(&at.distro).map_err(LaunchError::Wsl)?;
                let commands = profile::commands(start);
                let paths = [plugin_here.as_path()];
                let inside = wsl::inside(&at, distro, &commands, &self.shim, &paths);
                Site::wsl(inside.await.map_err(LaunchError::Wsl)?)
            }
            None => self.pc_site(),
        };
        let endpoint = self.endpoint(&token, site.inside());
        if let Some(at) = &endpoint {
            env.push((ENDPOINT_ENV, at.into()));
        }
        let account = accounted.and_then(|a| settings.account(a.cli.provider, site.distro()));
        if let Some(dir) = task {
            env.push((task::ENV, dir.shown.clone().into()));
        }
        if let Some(a) = accounted {
            env.extend(site.env(a.cli, account));
        }
        let inside = site.inside();
        let (plugin, skills) = match inside.as_ref().and_then(|i| i.path(0)) {
            Some(inside) => (
                PathBuf::from(inside),
                PathBuf::from(format!("{inside}/skills")),
            ),
            None => (plugin_here.clone(), self.plugin.skills(launch.skills)),
        };
        let default_choice = crate::protocol::ModelChoice::default();
        let at = LaunchAt {
            choice: match start {
                Start::Agent { choice, .. } => choice,
                Start::Command { .. } => &default_choice,
            },
            cwd,
            site: &site,
            inside,
            env: &env,
            plugin: &plugin,
            plugin_here: &plugin_here,
            run_dir: &self.run_dir,
            skills: &skills,
            task,
            launch,
            version: profile
                .and_then(|p| self.versions.get(&p))
                .map(String::as_str),
            node_ip: self.node_ip,
        };
        if let Some(p) = profile {
            profile::adapter(p).prepare(&at).map_err(internal)?;
        }
        let port = AgentPort::new(self.clone(), token.clone());
        let launched = profile::command(start, &at, &settings.bypass_off, port.clone()).await;
        let (mut cmd, started) = launched.inspect_err(|_| port.end())?;
        for (key, value) in &env {
            cmd.env(key, value);
        }
        if endpoint.is_none() {
            cmd.env_remove(ENDPOINT_ENV);
        }
        Ok(Prepared {
            cmd,
            inside: site.into_inside(),
            setup: None,
            token,
            account: account.cloned(),
            rules: profile.map(|p| profile::start_digests(p, launch)),
            profile,
            port,
            started,
            position,
            first_input: match start {
                Start::Agent { brief, .. } => !brief.trim().is_empty(),
                Start::Command { .. } => false,
            },
        })
    }

    pub(super) fn node_wsl(&self, node: Option<NodeId>) -> Result<Option<wsl::WslPath>, Problem> {
        let Some(node) = node else { return Ok(None) };
        self.wsl_at(node).map_err(internal)
    }

    pub(super) fn wsl_at(&self, node: NodeId) -> rusqlite::Result<Option<wsl::WslPath>> {
        let place = self.graph.place_of(node)?;
        Ok(wsl::of(std::path::Path::new(&place.worktree)))
    }

    pub(super) fn release_guarded(&self, node: NodeId, shells: &[Identity]) {
        if let Err(e) = self.graph.release_guarded(node, shells) {
            eprintln!("loom: 노드 {}의 멈춘 명령을 지우지 못했다: {e}", node.0);
        }
    }

    pub(super) fn end_guarded_inside(
        self: &Arc<Self>,
        node: NodeId,
        at: &wsl::WslPath,
        shells: Vec<Identity>,
    ) {
        if shells.is_empty() {
            return;
        }
        let token = self
            .graph
            .active()
            .ok()
            .and_then(|nodes| nodes.into_iter().find(|n| n.id == node))
            .map(|n| n.token);
        let Some(token) = token else { return };
        let Ok(distro) = self.distro(&at.distro) else {
            return;
        };
        let app = self.clone();
        tokio::spawn(async move {
            match wsl::end_shells(&distro, &token, &shells).await {
                Ok(()) => app.release_guarded(node, &shells),
                Err(e) => eprintln!("loom: 노드 {}의 명령을 멈추지 못했다: {e}", node.0),
            }
        });
    }

    pub(super) fn distro(&self, name: &str) -> Result<Distro, crate::protocol::InsideError> {
        self.distros
            .as_ref()
            .map(|distros| distros.distro(name))
            .ok_or_else(|| crate::protocol::InsideError::Start {
                distro: name.into(),
                cause: LINUX_LOOM_MISSING.into(),
            })
    }

    fn place_distro(&self, node: NodeId) -> Option<String> {
        self.node_wsl(Some(node)).ok().flatten().map(|at| at.distro)
    }

    fn endpoint(&self, token: &str, inside: Option<&wsl::Inside>) -> Option<String> {
        use sha2::Digest;
        let hash: String = sha2::Sha256::digest(token.as_bytes())[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        match inside {
            Some(inside) => Some(format!("{}/{hash}.sock", inside.run_dir())),
            None if cfg!(windows) => Some(format!(r"\\.\pipe\loom-{hash}")),
            None => Some(
                self.run_dir
                    .join(format!("{hash}.sock"))
                    .display()
                    .to_string(),
            ),
        }
    }

    fn node_env(&self, token: &str) -> Result<[(&'static str, OsString); 4], Problem> {
        Ok([
            ("LOOM_URL", self.url.clone().into()),
            ("LOOM_SPAWN_URL", self.spawn_url.clone().into()),
            (open::NODE_TOKEN_ENV, token.into()),
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
    pub(super) port: Arc<AgentPort>,
    pub(super) started: Started,
    pub(super) position: Position,
    pub(super) first_input: bool,
}

impl Prepared {
    pub(super) fn launched(&self) -> Launched {
        Launched {
            token: self.token.clone(),
            adapter: self.started.record.clone(),
            session: self.started.session.clone(),
            account: self.account.clone(),
            rules: self.rules.clone(),
        }
    }

    pub(super) fn shell(&self, line: &str) -> portable_pty::CommandBuilder {
        match &self.inside {
            Some(_) => portable_pty::CommandBuilder::from_argv(wsl::shell(line)),
            None => profile::shell(line),
        }
    }

    pub(super) fn discarder(&self) -> impl Fn() + use<> {
        let port = self.port.clone();
        move || port.end()
    }

    pub(super) async fn before_start(&mut self) -> Result<(), String> {
        match self.started.before_start.take() {
            Some(before) => before.await,
            None => Ok(()),
        }
    }
}

fn end_leftovers(runtime: &tokio::runtime::Handle, id: NodeId, distro: Distro, token: String) {
    runtime.spawn(async move {
        let names: Vec<&str> = std::iter::once(open::NODE_TOKEN_ENV)
            .chain(profile::SHELL_TOKEN_ENVS)
            .collect();
        if let Err(e) = wsl::end_token(&distro, &token, &names).await {
            eprintln!("loom: 노드 {}가 남긴 프로세스를 끝내지 못했다: {e}", id.0);
        }
    });
}

fn unserved_protocol(headers: &HeaderMap) -> Option<String> {
    let spoken = headers.get(NODE_CALLS_HEADER)?;
    let accepted = spoken
        .to_str()
        .ok()
        .and_then(|v| v.parse().ok())
        .is_some_and(|v| NODE_CALLS.accepts(v));
    (!accepted).then(|| String::from_utf8_lossy(spoken.as_bytes()).into_owned())
}

pub(super) async fn from_its_node(
    State(app): State<Arc<App>>,
    axum::extract::ConnectInfo(at): axum::extract::ConnectInfo<arrival::Caller>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match request.uri().path() == super::commands::IDLE_PATH {
        true => drop(request.extensions_mut().insert(at.arrival())),
        false => at.arrival().arrived(),
    }
    let caller = bearer(request.headers())
        .ok()
        .map(|token| app.graph.caller(token));
    match caller {
        Some(Ok(Some(node))) if node != at.node => (
            StatusCode::FORBIDDEN,
            "This node endpoint belongs to another node.",
        )
            .into_response(),
        _ => next.run(request).await,
    }
}

impl
    axum::extract::connect_info::Connected<
        axum::serve::IncomingStream<'_, tunnel::Door<arrival::Caller>>,
    > for arrival::Caller
{
    fn connect_info(
        stream: axum::serve::IncomingStream<'_, tunnel::Door<arrival::Caller>>,
    ) -> arrival::Caller {
        stream.remote_addr().clone()
    }
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

fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
