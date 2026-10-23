//! 노드 다시 띄우기: 노드의 에이전트 프로세스를 끝내고 지금 설정(계정 포함)으로 다시 띄워 같은 세션을 잇는다.
//! 턴 중이면 그 턴이 끝난 뒤 띄운다. 사람(노드 카드), 에이전트(`loom relaunch`), 계정 전환이 쓴다.
//! 프로필이 있는 에이전트(Claude, Codex, Cursor, OpenCode 2)만 된다. 셸·명령은 이을 세션이 없고 다시 띄우면 셸 상태를 잃는다.
//! 근거: docs/design.md 「다시 띄우기」, 「계정 전환」.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use ts_rs::TS;

use super::{AgentError, App, Problem, RelaunchRequest, for_agent, internal};
use crate::accounts::{self, Site};
use crate::graph::{Active, Delivery, Entry, Kind, NodeId, Refusal, Who};
use crate::process_tree::Snapshot;
use crate::profile::codex::{self, Status};
use crate::profile::cursor;
use crate::profile::{self, AgentId, ModelChoice, Profile, Provider, ServerLink, Start};
use crate::settings::SettingsView;
use crate::skills::Position;

/// 옛 노드 프로세스가 끝나기를 기다리는 상한. PTY 호스트는 끝내기 요청을 받으면 바로 프로세스 나무를 끝낸다.
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// `claude agents`·`claude stop`을 기다리는 상한. 넘으면 그대로 띄운다. 세션이 남았으면 새 CLI가 거절하고 그 실패가 기록에 남는다.
const CLAUDE_CLI_WAIT: Duration = Duration::from_secs(20);
/// 한도로 멈췄던 노드를 새 계정으로 이어 갈 때 넣는 첫 입력.
const CONTINUE_PROMPT: &str =
    "Your account was switched after a usage limit. Continue the task where you stopped.";
/// 끝난 노드를 되살릴 때 넣는 첫 입력. 끝난 동안 온 메시지는 받지 못했다(`Refusal::Ended`).
const REVIVED_NOTICE: &str = "Your process ended, and loom started it again in the same session. \
     Messages sent while it was ended did not arrive. Run `loom status`, then continue your work.";
/// 세션 기록 파일이 로그인 폴더 아래 몇 단계에 있는지. 새 폴더의 같은 상대 경로로 복사한다.
/// Claude: `projects/<작업 폴더>/<세션>.jsonl`. Codex: `sessions/<년>/<월>/<일>/rollout-…-<스레드>.jsonl`(0.159.2 실측).
const CLAUDE_TRANSCRIPT_DEPTH: usize = 3;
const CODEX_ROLLOUT_DEPTH: usize = 5;

/// 다시 띄우는 중이거나 턴 끝에 다시 띄울 노드. 기억에만 둔다. 데몬이 다시 시작하면 턴 끝을 기다리던 노드는 옛 프로세스로 남는다.
#[derive(Default)]
pub struct Relaunches {
    /// 옛 프로세스를 끝내는 중인 노드. 그 프로세스가 끝나면 `on_exit`가 신호를 보내고 노드를 `exited`로 두지 않는다.
    exiting: Mutex<HashMap<NodeId, oneshot::Sender<()>>>,
    /// 다시 띄울 노드와 그 차례. 이 동안 쉬는 대기와 서버 전달 작업은 메시지를 넘기지 않는다(`holds_mail`).
    pending: Mutex<HashMap<NodeId, Phase>>,
    /// 한도로 턴이 멈춘 노드. 다시 띄운 뒤 `CONTINUE_PROMPT`를 받는다.
    limited: Mutex<HashSet<NodeId>>,
}

/// 다시 띄울 노드의 차례.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    /// 턴이 끝나기를 기다린다.
    AtTurnEnd,
    /// 다시 띄우는 중이다.
    Running,
}

impl Relaunches {
    /// 다시 띄울 노드에는 메시지를 넘기지 않는다. 턴 끝에 옛 프로세스가 받은 메시지로 새 턴을 열면 다시 띄우기가 그 턴을 끊었다
    /// (2026-10-22 실측, Codex: 「Conversation interrupted」, 메시지는 전달됨으로 남았다). 새 프로세스가 받는다.
    pub(super) fn holds_mail(&self, node: NodeId) -> bool {
        self.pending.lock().unwrap().contains_key(&node)
    }

    /// 노드 프로세스가 끝났다. 다시 띄우기가 끝낸 프로세스면 기다리는 쪽에 알리고 참을 돌려준다.
    pub fn exited(&self, node: NodeId) -> bool {
        match self.exiting.lock().unwrap().remove(&node) {
            Some(done) => {
                let _ = done.send(());
                true
            }
            None => false,
        }
    }

    /// 옛 프로세스를 끝내는 중이다.
    pub(super) fn exiting(&self, node: NodeId) -> bool {
        self.exiting.lock().unwrap().contains_key(&node)
    }

    /// 노드가 한도로 턴을 멈췄다. 다시 띄울 때 이어 가라는 입력을 넣는다.
    pub(super) fn stopped_at_limit(&self, node: NodeId) {
        self.limited.lock().unwrap().insert(node);
    }
}

/// 다시 띄우기를 부른 결과.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Relaunched {
    /// 쉬던 노드다. 새 프로세스가 떴다.
    Now,
    /// 턴 중인 노드다. 그 턴이 끝나면 다시 띄운다.
    AtTurnEnd,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route(
        "/api/nodes/{id}/relaunch",
        get(relaunch_queued).post(relaunch_node),
    )
}

/// 그 노드가 다시 뜰 차례다(턴 끝을 기다리거나 다시 띄우는 중). 화면이 노드 메뉴를 열 때 묻는다.
async fn relaunch_queued(State(app): State<Arc<App>>, UrlPath(id): UrlPath<u32>) -> Json<bool> {
    Json(app.relaunches.holds_mail(NodeId(id)))
}

async fn relaunch_node(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<u32>,
) -> Result<Json<Relaunched>, Problem> {
    app.relaunch_by_id(NodeId(id)).await.map(Json)
}

/// `loom relaunch [<name>]`. 이름이 없으면 부른 노드 자신이다. 다른 노드는 부른 노드의 자식만 된다.
/// 부른 노드 자신은 턴 중이라 그 턴이 끝난 뒤 다시 뜬다. 그래서 응답이 먼저 간다.
pub(super) async fn agent_relaunch(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<RelaunchRequest>,
) -> Result<Json<Relaunched>, AgentError> {
    let caller = app.caller(&headers)?;
    let node = match &req.name {
        None => caller,
        Some(name) => {
            let (id, _) = app.graph.related(caller, name).map_err(for_agent)?;
            let parent = app.graph.relations(id).map_err(for_agent)?.parent;
            if parent.map(|p| p.id) != Some(caller) {
                return Err(for_agent(Refusal::NotChild(name.clone())));
            }
            id
        }
    };
    app.relaunch_by_id(node).await.map(Json).map_err(for_agent)
}

impl App {
    /// 서버 전달 작업(Codex, OpenCode 2)이 메일함에서 꺼낸다. 다시 띄울 노드에는 넘기지 않는다(`Relaunches::holds_mail`).
    pub(super) fn take_mail(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        if self.relaunches.holds_mail(node) {
            return Ok(Vec::new());
        }
        self.graph.take(node, kinds, limit_bytes)
    }

    /// 턴 중인 노드. Claude·Cursor는 턴 훅 뒤 턴 끝 훅이 오기 전이면, Codex는 스레드가 일하는 중이면,
    /// OpenCode 2는 세션 실행이 시작하고 끝나기 전이면 턴 중이다.
    pub(super) fn busy(&self, node: NodeId) -> bool {
        if let Some(server) = self.codex.lock().unwrap().get(&node) {
            return server
                .thread()
                .borrow()
                .as_ref()
                .is_some_and(|t| t.status == Status::Busy);
        }
        if let Some(session) = self.opencode2.lock().unwrap().get(&node) {
            return *session.working().borrow();
        }
        self.idle.in_turn(node)
    }

    /// 실행 중인 노드 하나를 다시 띄운다. 프로필이 없는 노드(셸, 명령)는 거절한다.
    /// 프로세스가 끝난 노드는 남긴 세션으로 되살린다.
    async fn relaunch_by_id(self: &Arc<Self>, id: NodeId) -> Result<Relaunched, Problem> {
        let found = self
            .graph
            .active()
            .map_err(internal)?
            .into_iter()
            .find(|n| n.id == id);
        let Some(node) = found else {
            return self.revive_ended(id).await;
        };
        if profile_of(&node).is_none() {
            return Err(Refusal::NoResume(node.name).into());
        }
        self.relaunch_or_queue(node).await.map_err(internal)
    }

    /// 끝난 노드는 턴이 없어 바로 띄운다. 두 번 누르면 둘째는 첫째를 기다리지 않고 돌아간다.
    async fn revive_ended(self: &Arc<Self>, id: NodeId) -> Result<Relaunched, Problem> {
        let node = self
            .graph
            .ended(id)
            .map_err(internal)?
            .ok_or_else(|| Refusal::Ended(format!("#{}", id.0)))?;
        if profile_of(&node).is_none() {
            return Err(Refusal::NoResume(node.name).into());
        }
        if self
            .relaunches
            .pending
            .lock()
            .unwrap()
            .insert(id, Phase::Running)
            .is_some()
        {
            return Ok(Relaunched::Now);
        }
        let who = Who {
            id: Some(id),
            name: node.name.clone(),
        };
        let app = self.clone();
        let result = tokio::spawn(async move { app.revive(node, REVIVED_NOTICE).await })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        self.relaunches.pending.lock().unwrap().remove(&id);
        self.record_relaunch(who, &result);
        result.map(|()| Relaunched::Now).map_err(internal)
    }

    /// 턴 중이면 턴 끝으로 미루고, 쉬면 바로 다시 띄운다.
    pub(super) async fn relaunch_or_queue(
        self: &Arc<Self>,
        node: Active,
    ) -> Result<Relaunched, String> {
        // 두 번 누르면 둘째가 첫째의 종료 신호를 바꿔 쥐어, 첫째가 옛 프로세스가 살아 있는 채로 새 프로세스를 띄운다.
        let phase = {
            let mut pending = self.relaunches.pending.lock().unwrap();
            if let Some(phase) = pending.get(&node.id) {
                return Ok(match phase {
                    Phase::AtTurnEnd => Relaunched::AtTurnEnd,
                    Phase::Running => Relaunched::Now,
                });
            }
            let phase = match self.busy(node.id) {
                true => Phase::AtTurnEnd,
                false => Phase::Running,
            };
            pending.insert(node.id, phase);
            phase
        };
        if phase == Phase::AtTurnEnd {
            self.watch_server_turn(node.id);
            return Ok(Relaunched::AtTurnEnd);
        }
        // 부른 쪽이 끊겨도 끝까지 한다. 옛 프로세스를 끝낸 뒤 멈추면 노드가 새 프로세스 없이 남는다.
        let app = self.clone();
        tokio::spawn(async move { app.relaunch(node).await })
            .await
            .map_err(|e| e.to_string())??;
        Ok(Relaunched::Now)
    }

    /// 턴이 끝났다(Claude·Cursor 쉬는 대기가 열렸다, Codex 스레드가 쉰다, OpenCode 2 실행이 끝났다). 다시 띄울 노드면 띄운다.
    pub(super) fn turn_ended(self: &Arc<Self>, node: NodeId) {
        {
            let mut pending = self.relaunches.pending.lock().unwrap();
            match pending.get_mut(&node) {
                Some(phase @ Phase::AtTurnEnd) => *phase = Phase::Running,
                _ => return,
            }
        }
        let app = self.clone();
        tokio::spawn(async move {
            let found = app.graph.active().map(|nodes| {
                nodes
                    .into_iter()
                    .find(|n| n.id == node)
                    .filter(|n| profile_of(n).is_some())
            });
            let relaunched = match found {
                Ok(Some(active)) => app.relaunch(active).await,
                Ok(None) => {
                    app.relaunches.pending.lock().unwrap().remove(&node);
                    return;
                }
                Err(e) => {
                    app.relaunches.pending.lock().unwrap().remove(&node);
                    Err(e.to_string())
                }
            };
            if let Err(e) = relaunched {
                eprintln!("loom: 노드 {}를 턴 끝에 다시 띄우지 못했다: {e}", node.0);
            }
        });
    }

    /// Codex·OpenCode 2 노드는 턴 끝 훅이 없다. 서버가 쉰다고 알리면 `turn_ended`를 부른다.
    fn watch_server_turn(self: &Arc<Self>, node: NodeId) {
        let codex = self.codex.lock().unwrap().get(&node).cloned();
        let opencode2 = self.opencode2.lock().unwrap().get(&node).cloned();
        let app = self.clone();
        tokio::spawn(async move {
            let idle = match (codex, opencode2) {
                (Some(server), _) => {
                    let mut thread = server.thread();
                    let idle =
                        thread.wait_for(|t| t.as_ref().is_none_or(|t| t.status == Status::Idle));
                    idle.await.is_ok()
                }
                (_, Some(session)) => session.working().wait_for(|w| !w).await.is_ok(),
                (None, None) => return,
            };
            if idle {
                app.turn_ended(node);
            }
        });
    }

    /// 다시 띄우고 그 결과를 이벤트 기록에 남긴다.
    async fn relaunch(self: &Arc<Self>, node: Active) -> Result<(), String> {
        let who = Who {
            id: Some(node.id),
            name: node.name.clone(),
        };
        let id = node.id;
        let result = self.restart(node).await;
        self.relaunches.pending.lock().unwrap().remove(&id);
        self.record_relaunch(who, &result);
        result
    }

    fn record_relaunch(&self, node: Who, result: &Result<(), String>) {
        let entry = Entry::Relaunched {
            node,
            error: result.as_ref().err().cloned(),
        };
        if let Err(e) = self.graph.record(&entry) {
            eprintln!("loom: 다시 띄운 결과를 남기지 못했다: {e}");
        }
    }

    /// 노드 프로세스를 지금 설정(계정·실행 설정)으로 다시 띄운다. 계정이 바뀌었으면 새 로그인 폴더에 세션 기록을 복사하고 이어 간다.
    /// 남긴 세션이 없으면(이 기능 전에 뜬 노드가 아직 훅을 부르지 않았다, 첫 입력 전) 다시 띄우지 않는다.
    /// 새로 띄우면 사람이 친 대화와 첫 지시를 잃는다. 새 노드를 준비하지 못해도 옛 프로세스를 그대로 둔다.
    async fn restart(self: &Arc<Self>, node: Active) -> Result<(), String> {
        let (Some(agent), Some(profile)) = (node.agent.clone(), profile_of(&node)) else {
            return Err(format!(
                "노드 {}는 프로필이 있는 에이전트가 아니다",
                node.id.0
            ));
        };
        let settings = self.settings.get().map_err(|e| e.to_string())?;
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let cwd = Path::new(&place.worktree);
        let resume = match profile {
            Profile::Claude | Profile::Codex => {
                self.carry_session(&node, profile, &settings, cwd).await?
            }
            Profile::Cursor => session_id(node.transcript.as_deref().map(Path::new))
                .ok_or("이어 갈 Cursor 세션 기록이 없다")?,
            Profile::OpenCode2 => match &node.server {
                Some(ServerLink::OpenCode2 { session, .. }) => session.clone(),
                _ => return Err("이어 갈 OpenCode 2 세션이 없다".into()),
            },
        };
        let limited = self.relaunches.limited.lock().unwrap().contains(&node.id);
        let first = match (limited, profile) {
            (true, _) => Some(CONTINUE_PROMPT.to_owned()),
            (false, Profile::Cursor) => Some(cursor::RESUMED_PROMPT.to_owned()),
            (false, _) => None,
        };
        // Claude Code는 이어 간 세션에 끝나지 않은 백그라운드 명령을 스스로 알린다(2.1.308 실측). 다른 CLI는 알리지 않았다(실측).
        let stopped = match profile {
            Profile::Claude => 0,
            Profile::Codex | Profile::Cursor | Profile::OpenCode2 => {
                let snapshot = Snapshot::now();
                let loom = format!("loom{}", std::env::consts::EXE_SUFFIX);
                let shells = self.background_shells(&snapshot, &node);
                shells
                    .into_iter()
                    .filter(|&s| !snapshot.runs(s, &loom))
                    .count()
            }
        };
        let stopped = (stopped > 0).then(|| stopped_notice(stopped));
        let brief = first
            .into_iter()
            .chain(stopped)
            .collect::<Vec<_>>()
            .join("\n");
        let prepared = self
            .prepare_resumed(&node, &agent, profile, &resume, brief)
            .await?;
        let discard = prepared.discarder();
        // 옛 프로세스가 끝난 뒤 띄운다. 옛 PTY 호스트가 끝나며 지우는 주소 파일을 새 호스트가 함께 쓴다.
        let (done, exited) = oneshot::channel();
        self.relaunches
            .exiting
            .lock()
            .unwrap()
            .insert(node.id, done);
        // 옛 프로세스가 띄운 백그라운드 명령도 끝낸다. Cursor는 노드 프로세스가 끝나도 그 명령이 고아로 남았다(실측).
        let snapshot = Snapshot::now();
        snapshot.kill_trees(self.background_shells(&snapshot, &node));
        // 옛 CLI의 쉬는 대기를 끝낸다. 새 CLI가 세션을 시작하면 다시 연다.
        self.idle.end(node.id);
        self.host.kill(node.id);
        if tokio::time::timeout(EXIT_WAIT, exited).await.is_err() {
            self.relaunches.exiting.lock().unwrap().remove(&node.id);
            discard();
            return Err(format!("노드 {}의 옛 프로세스가 끝나지 않았다", node.id.0));
        }
        if profile == Profile::Claude {
            self.stop_claude_background(&node, &resume).await;
        }
        self.run_resumed(node.id, prepared, &resume, &settings, &agent)
            .await
    }

    /// 프로세스가 없는 노드를 남긴 세션으로 다시 띄운다. 옛 WSL 데몬에서 옮겨 온 노드가 쓴다(`carry`).
    /// 계정은 지금 설정의 것이다. 세션 기록은 그 계정 폴더로 옮긴다(Claude). Codex는 남긴 스레드를 그대로 잇는다.
    pub(super) async fn revive(self: &Arc<Self>, node: Active, notice: &str) -> Result<(), String> {
        let (Some(agent), Some(profile)) = (node.agent.clone(), profile_of(&node)) else {
            return Err("프로필이 있는 에이전트가 아니다".into());
        };
        let settings = self.settings.get().map_err(|e| e.to_string())?;
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let resume = match (profile, &node.server) {
            (Profile::Claude, _) => {
                self.carry_session(&node, profile, &settings, Path::new(&place.worktree))
                    .await?
            }
            (Profile::Cursor, _) => session_id(node.transcript.as_deref().map(Path::new))
                .ok_or("이어 갈 Cursor 세션 기록이 없다")?,
            (
                Profile::Codex,
                Some(ServerLink::Codex {
                    thread: Some(thread),
                    ..
                }),
            ) => thread.clone(),
            (Profile::OpenCode2, Some(ServerLink::OpenCode2 { session, .. })) => session.clone(),
            (Profile::Codex | Profile::OpenCode2, _) => {
                return Err("이어 갈 서버 세션이 없다".into());
            }
        };
        let prepared = self
            .prepare_resumed(&node, &agent, profile, &resume, notice.to_owned())
            .await?;
        if profile == Profile::Claude {
            self.stop_claude_background(&node, &resume).await;
        }
        self.run_resumed(node.id, prepared, &resume, &settings, &agent)
            .await
    }

    async fn prepare_resumed(
        &self,
        node: &Active,
        agent: &AgentId,
        profile: Profile,
        resume: &str,
        brief: String,
    ) -> Result<super::Prepared, String> {
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let start = Start::Agent {
            agent: agent.clone(),
            brief,
            choice: match profile {
                // OpenCode 2 TUI는 모델 인자를 받지 않는다(`LaunchError::NoModelChoice`). 모델은 세션에 남아 이어진다.
                Profile::OpenCode2 => ModelChoice::default(),
                Profile::Claude | Profile::Codex | Profile::Cursor => ModelChoice {
                    model: node.model.model.clone(),
                    effort: node.model.effort.clone(),
                },
            },
            resume: Some(resume.to_owned()),
        };
        let position = match self.graph.relations(node.id) {
            Ok(r) if r.parent.is_some() => Position::Child,
            Ok(_) => Position::Root,
            Err(e) => return Err(e.to_string()),
        };
        self.prepare(&start, Path::new(&place.worktree), position)
            .await
            .map_err(|e| format!("{e:?}"))
    }

    async fn run_resumed(
        self: &Arc<Self>,
        id: NodeId,
        prepared: super::Prepared,
        resume: &str,
        settings: &SettingsView,
        agent: &AgentId,
    ) -> Result<(), String> {
        let discard = prepared.discarder();
        // 옛 app-server가 끝난 뒤 새 서버에서 스레드를 연다. 같은 `CODEX_HOME`이면 옛 서버가 스레드를 쥐고 있어
        // "thread/resume: thread … already has an active writer"로 실패했다(0.159.2 실측, 계정이 같은 다시 띄우기).
        if let Some(server) = &prepared.codex {
            let bypass = !settings.bypass_off.contains(agent);
            let call = |method, params| server.call(method, params);
            if let Err(e) = codex::resume(call, resume, bypass).await {
                discard();
                let _ = self.graph.lost(id);
                return Err(e);
            }
            server.follow_thread(resume.to_owned()).await;
        }
        // 새 토큰을 먼저 남긴다. 새 프로세스가 뜨자마자 데몬을 불러도 찾는다.
        if let Err(e) = self.graph.relaunched(id, &prepared.launched()) {
            discard();
            let _ = self.graph.lost(id);
            return Err(e.to_string());
        }
        // 새 프로세스의 전달 작업이 처음부터 메시지를 꺼내게 한다.
        self.relaunches.pending.lock().unwrap().remove(&id);
        if let Err(e) = self.launch(id, prepared) {
            discard();
            // 옛 프로세스는 이미 끝났다. 화면이 실행 중으로 남지 않게 한다.
            let _ = self.graph.lost(id);
            return Err(e);
        }
        self.relaunches.limited.lock().unwrap().remove(&id);
        // 옛 프로세스의 창은 사라졌다. 새 프로세스가 다시 묻으면 신호가 다시 건다.
        self.graph.clear_wait(id).map_err(|e| e.to_string())
    }

    /// 노드의 위치(이 PC 또는 그 WSL 배포판)에서 그 위치의 「사용 중」 계정 로그인 폴더로 세션 기록을 옮기고 세션 id를 돌려준다.
    /// 세션 기록도 그 위치의 폴더에 있다. 계정이 같으면 복사하지 않는다.
    async fn carry_session(
        &self,
        node: &Active,
        profile: Profile,
        settings: &SettingsView,
        cwd: &Path,
    ) -> Result<String, String> {
        let provider = profile.provider().ok_or("계정이 없는 프로필이다")?;
        let site = self.site_at(cwd).await.map_err(|e| format!("{e:?}"))?;
        let target = settings.account(provider, site.distro()).cloned();
        let new_root = site
            .login_dir(provider, target.as_ref())
            .ok_or("홈 폴더를 찾지 못했다")?;
        if profile == Profile::Codex {
            return self.copy_codex_thread(node.id, &site, &new_root).await;
        }
        let transcript = node.transcript.as_deref().map(|t| site.local(t));
        let session = copy_claude_session(transcript.as_deref(), &new_root)?;
        // 시스템 기본 폴더는 고치지 않는다(「저장」). 그때 사람이 새 확인 창에 답한다.
        if let (Some(to), Some(from)) = (
            site.accounts().config_dir(provider, target.as_ref()),
            site.claude_files(node.account.as_ref()),
        ) {
            accounts::carry_claude_consent(&from, &to, &site.cli_cwd(cwd))
                .map_err(|e| format!("{}: {e}", to.display()))?;
        }
        Ok(session)
    }

    /// Claude Code가 세션을 자기 백그라운드 데몬으로 옮겼으면 그 세션을 끝낸다. 대화는 남는다.
    /// 그 세션은 노드 프로세스 나무 밖에 있어 PTY 호스트를 끝내도 산다. 살아 있으면 새 CLI의 `--resume`이
    /// "That session is running in the background"로 거절하고 코드 1로 끝났다(2026-10-23 실측, 2.1.308).
    /// `claude agents --json`에서 전체 세션 id가 같은 백그라운드 작업을 찾을 때만 끝낸다. 못 찾으면 아무것도 끝내지 않는다.
    /// 세션은 노드를 띄운 계정에 있다.
    async fn stop_claude_background(&self, node: &Active, session: &str) {
        let Ok(place) = self.graph.place_of(node.id) else {
            return;
        };
        let Ok(site) = self.site_at(Path::new(&place.worktree)).await else {
            return;
        };
        let account = node.account.as_ref();
        let Ok(list) = site.command(Provider::Claude, &["agents", "--json"], account) else {
            return;
        };
        let Some(out) = run_claude(list).await else {
            return;
        };
        let Some(job) = background_job(&out, session) else {
            return;
        };
        if let Ok(stop) = site.command(Provider::Claude, &["stop", &job], account) {
            run_claude(stop).await;
        }
    }

    /// 옛 프로세스가 띄운 셸: 노드 프로세스와 그 서버(Codex app-server, OpenCode 2 serve) 아래의 세션 리더.
    /// CLI는 셸을 새 세션으로 띄운다(`CodexServer::stop_shells`와 같은 판단). 쉬는 노드라 앞에서 도는 명령은 없다.
    /// `loom`을 도는 셸은 쉬는 대기 훅(Claude `loom hook`, Cursor `loom cursor-hook`)이다(실측). 끝내되 명령으로 세지 않는다.
    fn background_shells(&self, snapshot: &Snapshot, node: &Active) -> Vec<u32> {
        let server = match &node.server {
            Some(ServerLink::Codex { pid, .. } | ServerLink::OpenCode2 { pid, .. }) => *pid,
            None => None,
        };
        [self.host.pid(node.id), server]
            .into_iter()
            .flatten()
            .flat_map(|root| snapshot.session_leaders_under(root))
            .collect()
    }

    /// Codex 노드 스레드의 기록 파일을 새 `CODEX_HOME`에 복사하고 스레드 id를 돌려준다. 스레드나 기록 파일이 없으면 실패다.
    async fn copy_codex_thread(
        &self,
        node: NodeId,
        site: &Site,
        new_root: &Path,
    ) -> Result<String, String> {
        let server = self.codex.lock().unwrap().get(&node).cloned();
        let thread = server
            .as_ref()
            .and_then(|s| s.thread().borrow().as_ref().map(|t| t.id.clone()));
        let (Some(server), Some(thread)) = (server, thread) else {
            return Err(format!("노드 {}에는 이어 갈 Codex 스레드가 없다", node.0));
        };
        let rollout = server
            .rollout(&thread)
            .await?
            .ok_or_else(|| format!("스레드 {thread}의 기록 파일이 없다"))?;
        copy_session(&site.local(&rollout), CODEX_ROLLOUT_DEPTH, new_root)?;
        Ok(thread)
    }
}

fn profile_of(node: &Active) -> Option<Profile> {
    node.agent.as_ref().and_then(profile::profile_of)
}

pub(super) fn provider_of(node: &Active) -> Option<Provider> {
    profile_of(node).and_then(Profile::provider)
}

/// 다시 띄운 세션의 첫 입력에 넣는 줄. 에이전트는 끝난 명령의 결과를 기다리지 않고 필요하면 다시 띄운다.
fn stopped_notice(count: usize) -> String {
    format!(
        "loom relaunched your process and stopped {count} background command(s) that you started before. Start them again if you still need them."
    )
}

/// 세션 id는 세션 기록 파일 이름이다(Claude `<세션>.jsonl`, Cursor `agent-transcripts/<세션>/<세션>.jsonl`).
fn session_id(transcript: Option<&Path>) -> Option<String> {
    transcript?
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

/// Claude 세션 기록과 그 곁 폴더(`<세션>/`, 하위 에이전트 기록)를 새 로그인 폴더에 복사하고 세션 id를 돌려준다.
/// 기록 파일이 없으면 실패다. 그 노드는 옮기지 않는다. 세션 id는 기록 파일 이름이다.
fn copy_claude_session(transcript: Option<&Path>, new_root: &Path) -> Result<String, String> {
    let file = transcript
        .filter(|f| f.is_file())
        .ok_or("이어 갈 Claude 세션 기록이 없다")?;
    let session = session_id(Some(file)).ok_or("세션 기록 파일 이름이 없다")?;
    copy_session(file, CLAUDE_TRANSCRIPT_DEPTH, new_root)?;
    let side = file.with_extension("");
    if side.is_dir()
        && let Some(to) = relocate(&side, CLAUDE_TRANSCRIPT_DEPTH, new_root)
        && to != side
    {
        copy_dir(&side, &to).map_err(|e| format!("{}: {e}", side.display()))?;
    }
    Ok(session)
}

fn copy_session(file: &Path, depth: usize, new_root: &Path) -> Result<(), String> {
    let to = relocate(file, depth, new_root)
        .ok_or_else(|| format!("{}: 로그인 폴더 아래 기록 파일이 아니다", file.display()))?;
    if to == file {
        return Ok(());
    }
    let copied = to
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| copy_file(file, &to));
    copied
        .map(drop)
        .map_err(|e| format!("{} → {}: {e}", file.display(), to.display()))
}

/// `file`의 끝 `depth` 단계를 `new_root` 아래 같은 상대 경로로 옮긴 경로.
fn relocate(file: &Path, depth: usize, new_root: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = file.components().collect();
    let tail = parts.get(parts.len().checked_sub(depth)?..)?;
    Some(tail.iter().fold(new_root.to_path_buf(), |p, c| p.join(c)))
}

/// 읽은 뒤 쓴다. 두 경로가 링크로 같은 파일이어도(옛 WSL 데몬의 `HOME`이 링크였다) 내용을 잃지 않는다. `fs::copy`는 대상을 먼저 비운다.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::write(to, std::fs::read(from)?)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Claude CLI를 돌려 성공하면 표준 출력을 준다. `CLAUDE_CLI_WAIT`을 넘기면 끝내고 없다고 본다.
async fn run_claude(cmd: std::process::Command) -> Option<Vec<u8>> {
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(CLAUDE_CLI_WAIT, cmd.output())
        .await
        .ok()?
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// `claude agents --json` 한 줄(2.1.309 실측 필드).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeJob {
    id: Option<String>,
    kind: String,
    session_id: String,
}

/// `claude agents --json` 출력에서 전체 세션 id가 `session`인 백그라운드 작업의 id(`claude stop`이 받는 짧은 id)를 찾는다.
/// `claude stop`은 전체 UUID를 받지 않고("No job matching"), 짧은 id는 앞 8글자라 다른 세션과 겹칠 수 있다.
/// 그래서 전체 id로 주인을 확인한다. 읽지 못하거나 맞는 작업이 없으면 없다.
fn background_job(json: &[u8], session: &str) -> Option<String> {
    let jobs: Vec<ClaudeJob> = serde_json::from_slice(json).ok()?;
    jobs.into_iter()
        .find(|j| j.kind == "background" && j.session_id == session)?
        .id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_background_job_of_the_same_full_session_is_stopped() {
        let json = br#"[
            {"pid":1,"id":"925fee83","kind":"background","sessionId":"925fee83-0000-4000-8000-000000000002","status":"idle"},
            {"pid":2,"kind":"interactive","sessionId":"aaaaaaaa-0000-4000-8000-000000000001"}
        ]"#;
        let mine = "aaaaaaaa-0000-4000-8000-000000000001";
        assert_eq!(
            background_job(json, "925fee83-ffff-4000-8000-00000000000f"),
            None
        );
        assert_eq!(background_job(json, mine), None);
        assert_eq!(
            background_job(json, "925fee83-0000-4000-8000-000000000002").as_deref(),
            Some("925fee83")
        );
        assert_eq!(background_job(b"not json", mine), None);
        assert_eq!(background_job(b"[]", mine), None);
    }

    #[test]
    fn session_files_keep_their_path_under_the_new_login_folder() {
        assert_eq!(
            relocate(
                Path::new("/h/.claude/projects/-w/abc.jsonl"),
                CLAUDE_TRANSCRIPT_DEPTH,
                Path::new("/l/acc")
            ),
            Some(PathBuf::from("/l/acc/projects/-w/abc.jsonl"))
        );
        assert_eq!(
            relocate(
                Path::new("/h/.codex/sessions/2026/10/21/rollout-x-t1.jsonl"),
                CODEX_ROLLOUT_DEPTH,
                Path::new("/l/c")
            ),
            Some(PathBuf::from("/l/c/sessions/2026/10/21/rollout-x-t1.jsonl"))
        );
        assert_eq!(relocate(Path::new("a.jsonl"), 3, Path::new("/l")), None);
    }

    /// Cursor는 `--resume <세션>`으로 잇는다. 세션 id는 훅이 알린 기록 파일 이름이다(2026.10.19 실측 경로).
    #[test]
    fn session_id_is_the_transcript_file_name() {
        let cursor =
            Path::new("/h/.cursor/projects/tmp-w/agent-transcripts/93f2-8533/93f2-8533.jsonl");
        assert_eq!(session_id(Some(cursor)).as_deref(), Some("93f2-8533"));
        assert_eq!(session_id(None), None);
    }

    #[test]
    fn claude_session_copies_the_transcript_and_its_side_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old/projects/-w");
        std::fs::create_dir_all(old.join("s1/subagents")).unwrap();
        std::fs::write(old.join("s1.jsonl"), "x").unwrap();
        std::fs::write(old.join("s1/subagents/a.jsonl"), "y").unwrap();
        let new = tmp.path().join("new");
        let transcript = old.join("s1.jsonl");
        assert_eq!(copy_claude_session(Some(&transcript), &new).unwrap(), "s1");
        assert_eq!(
            std::fs::read_to_string(new.join("projects/-w/s1.jsonl")).unwrap(),
            "x"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("projects/-w/s1/subagents/a.jsonl")).unwrap(),
            "y"
        );
    }

    /// 남긴 세션이 없는 노드(이 기능 전에 뜬 노드, 첫 입력 전)를 새로 띄우면 대화를 잃는다. 실패로 세고 아무것도 쓰지 않는다.
    #[test]
    fn no_stored_session_fails_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let new = tmp.path().join("new");
        assert!(copy_claude_session(None, &new).is_err());
        let missing = tmp.path().join("old/projects/-w/gone.jsonl");
        assert!(copy_claude_session(Some(&missing), &new).is_err());
        assert!(!new.exists());
    }
}
