//! 계정 전환: 실행 중인 노드를 새 계정으로 다시 띄워 같은 세션을 잇고, 켜져 있으면 한도에 닿은 계정을 자동으로 바꾼다.
//! 근거: docs/design.md 「계정 전환」, 목업 docs/mockups/account-switch.html.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use ts_rs::TS;

use super::{App, Problem, internal};
use crate::accounts::{self, AccountId};
use crate::graph::{Active, Entry, NodeId, SwitchCause};
use crate::profile::codex::{self, Status};
use crate::profile::{self, ModelChoice, Profile, Provider, Start};
use crate::skills::Position;
use crate::usage::{self, Limits, Window};

/// 이 비율(%)을 쓴 창이 있는 계정은 다 썼다. 그 계정으로는 바꾸지 않는다.
const EXHAUSTED_AT: u8 = 100;
/// 같은 공급자를 다시 자동 전환하지 않는 시간. 같은 계정의 노드 여럿이 한꺼번에 한도로 멈추면
/// 멈춤마다 다시 바꾸어 계정을 오가는 일을 막는다. 「남은 계정 없음」 기록도 이 간격에 한 번만 남긴다.
const COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// 옛 노드 프로세스가 끝나기를 기다리는 상한. PTY 호스트는 끝내기 요청을 받으면 바로 프로세스 나무를 끝낸다.
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// 한도로 멈췄던 노드를 새 계정으로 이어 갈 때 넣는 첫 입력.
const CONTINUE_PROMPT: &str =
    "Your account was switched after a usage limit. Continue the task where you stopped.";
/// 세션 기록 파일이 로그인 폴더 아래 몇 단계에 있는지. 새 폴더의 같은 상대 경로로 복사한다.
/// Claude: `projects/<작업 폴더>/<세션>.jsonl`. Codex: `sessions/<년>/<월>/<일>/rollout-…-<스레드>.jsonl`(0.159.2 실측).
const CLAUDE_TRANSCRIPT_DEPTH: usize = 3;
const CODEX_ROLLOUT_DEPTH: usize = 5;

/// 계정을 바꿀 때 옮길 수 있는 실행 중인 노드(`GET /api/accounts/{provider}/nodes`). 확인 창 목록이다(목업 ①).
/// 지금 계정이 설정의 계정과 같은 노드는 넣지 않는다.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MovableNode {
    pub id: NodeId,
    pub name: String,
    /// 턴 중이다. 그 턴이 끝난 뒤 옮긴다.
    pub busy: bool,
}

/// `POST /api/accounts/{provider}/move`의 결과. 설정에 저장한 계정으로 옮긴다. 같은 값을 이벤트 기록에도 남긴다.
#[derive(Clone, Copy, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MoveResult {
    pub moved: u32,
    pub later: u32,
    pub failed: u32,
}

/// 옮기는 중이거나 옮길 노드. 기억에만 둔다. 데몬이 다시 시작하면 턴 끝을 기다리던 노드는 옛 계정에 남는다.
#[derive(Default)]
pub struct Switches {
    /// 옛 프로세스를 끝내는 중인 노드. 그 프로세스가 끝나면 `on_exit`가 신호를 보내고 노드를 `exited`로 두지 않는다.
    exiting: Mutex<HashMap<NodeId, oneshot::Sender<()>>>,
    /// 턴이 끝나면 옮길 노드.
    later: Mutex<HashSet<NodeId>>,
    /// 한도로 턴이 멈춘 노드. 옮긴 뒤 `CONTINUE_PROMPT`를 받는다.
    limited: Mutex<HashSet<NodeId>>,
    /// 공급자마다 마지막 자동 전환이나 「남은 계정 없음」 기록 시각.
    last_auto: Mutex<HashMap<Provider, Instant>>,
}

impl Switches {
    /// 노드 프로세스가 끝났다. 계정 전환이 끝낸 프로세스면 기다리는 쪽에 알리고 참을 돌려준다.
    pub fn exited(&self, node: NodeId) -> bool {
        match self.exiting.lock().unwrap().remove(&node) {
            Some(done) => {
                let _ = done.send(());
                true
            }
            None => false,
        }
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/accounts/{provider}/nodes", get(movable))
        .route("/api/accounts/{provider}/move", post(move_all))
}

/// `GET /api/accounts/{provider}/nodes`의 질의. `to`는 옮겨 갈 계정이다. 없으면 시스템 기본이다.
/// 화면은 설정을 저장하기 전에 확인 창 목록을 읽는다.
#[derive(Deserialize)]
struct MovableQuery {
    to: Option<AccountId>,
}

async fn movable(
    State(app): State<Arc<App>>,
    UrlPath(provider): UrlPath<Provider>,
    Query(query): Query<MovableQuery>,
) -> Result<Json<Vec<MovableNode>>, Problem> {
    let nodes = app.movable(provider, query.to.as_ref()).map_err(internal)?;
    Ok(Json(
        nodes
            .into_iter()
            .map(|n| MovableNode {
                busy: app.busy(n.id),
                id: n.id,
                name: n.name,
            })
            .collect(),
    ))
}

/// 화면이 설정을 먼저 저장한 뒤 부른다.
async fn move_all(
    State(app): State<Arc<App>>,
    UrlPath(provider): UrlPath<Provider>,
) -> Result<Json<MoveResult>, Problem> {
    Ok(Json(app.switch_nodes(provider, SwitchCause::Manual).await?))
}

/// 노드 하나를 옮긴 결과.
enum Moved {
    Now,
    Later,
    Failed,
}

impl App {
    /// 실행 중인 그 공급자 노드 가운데 `target` 계정과 다른 계정으로 띄운 노드. `None`은 시스템 기본이다.
    fn movable(
        &self,
        provider: Provider,
        target: Option<&AccountId>,
    ) -> rusqlite::Result<Vec<Active>> {
        let exiting: HashSet<NodeId> = self
            .switch
            .exiting
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();
        Ok(self
            .graph
            .active()?
            .into_iter()
            .filter(|n| provider_of(n) == Some(provider))
            .filter(|n| n.account.as_ref() != target && !exiting.contains(&n.id))
            .collect())
    }

    /// 턴 중인 노드. Claude는 쉬는 대기가 없으면, Codex는 스레드가 일하는 중이면 턴 중이다.
    fn busy(&self, node: NodeId) -> bool {
        match self.codex.lock().unwrap().get(&node) {
            Some(server) => server
                .thread()
                .borrow()
                .as_ref()
                .is_some_and(|t| t.status == Status::Busy),
            None => !self.idle.is_open(node),
        }
    }

    /// 옮길 노드를 모두 설정의 계정으로 옮기고 이벤트 기록에 한 줄을 남긴다.
    /// `from`은 옮길 노드가 가장 많이 쓰던 계정이다. 같은 수면 번호가 작은 노드의 계정이다.
    async fn switch_nodes(
        self: &Arc<Self>,
        provider: Provider,
        why: SwitchCause,
    ) -> Result<MoveResult, Problem> {
        let to = self
            .settings
            .get()
            .map_err(internal)?
            .account(provider)
            .cloned();
        let nodes = self.movable(provider, to.as_ref()).map_err(internal)?;
        let from = most_used(nodes.iter().map(|n| n.account.clone()));
        let outcomes = join_all(nodes.into_iter().map(|node| self.move_or_queue(node))).await;
        let mut result = MoveResult::default();
        for outcome in outcomes {
            match outcome {
                Moved::Now => result.moved += 1,
                Moved::Later => result.later += 1,
                Moved::Failed => result.failed += 1,
            }
        }
        self.graph
            .record(&Entry::AccountSwitched {
                provider,
                from,
                to,
                why,
                moved: result.moved,
                later: result.later,
                failed: result.failed,
            })
            .map_err(internal)?;
        Ok(result)
    }

    async fn move_or_queue(self: &Arc<Self>, node: Active) -> Moved {
        if self.busy(node.id) {
            self.switch.later.lock().unwrap().insert(node.id);
            self.watch_codex_turn(node.id);
            return Moved::Later;
        }
        match self.relaunch(node).await {
            Ok(()) => Moved::Now,
            Err(e) => {
                eprintln!("loom: 노드를 새 계정으로 옮기지 못했다: {e}");
                Moved::Failed
            }
        }
    }

    /// 턴이 끝났다(Claude 쉬는 대기가 열렸다, Codex 스레드가 쉰다). 옮길 노드면 옮긴다.
    pub(super) fn turn_ended(self: &Arc<Self>, node: NodeId) {
        if !self.switch.later.lock().unwrap().remove(&node) {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let found = app.graph.active().map(|nodes| {
                nodes
                    .into_iter()
                    .find(|n| n.id == node)
                    .filter(|n| provider_of(n).is_some())
            });
            let relaunched = match found {
                Ok(Some(active)) => app.relaunch(active).await,
                Ok(None) => return,
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = relaunched {
                eprintln!("loom: 노드 {}를 턴 끝에 옮기지 못했다: {e}", node.0);
            }
        });
    }

    /// Codex 노드는 훅이 없다. 스레드가 쉬면 `turn_ended`를 부른다.
    fn watch_codex_turn(self: &Arc<Self>, node: NodeId) {
        let Some(server) = self.codex.lock().unwrap().get(&node).cloned() else {
            return;
        };
        let app = self.clone();
        tokio::spawn(async move {
            let mut thread = server.thread();
            let idle = thread.wait_for(|t| t.as_ref().is_none_or(|t| t.status == Status::Idle));
            if idle.await.is_ok() {
                app.turn_ended(node);
            }
        });
    }

    /// 노드가 한도로 턴을 멈췄다. 옮길 때 이어 가라는 입력을 넣는다. 자동 전환이 켜져 있으면 바로 바꾼다.
    /// 자동 전환은 이 이벤트로만 시작한다. 사용량을 미리 읽어 바꾸지 않는다.
    pub(super) fn rate_limited(self: &Arc<Self>, node: NodeId, provider: Provider) {
        self.switch.limited.lock().unwrap().insert(node);
        let app = self.clone();
        tokio::spawn(async move { app.auto_switch(provider).await });
    }

    /// 노드 프로세스를 설정의 계정으로 다시 띄운다. 새 로그인 폴더에 세션 기록을 복사하고 이어 간다.
    /// 남긴 세션이 없으면(이 기능 전에 뜬 노드가 아직 훅을 부르지 않았다, Codex 스레드 전) 옮기지 않는다.
    /// 새로 띄우면 사람이 친 대화와 첫 지시를 잃는다. 새 노드를 준비하지 못해도 옛 프로세스를 그대로 둔다.
    async fn relaunch(self: &Arc<Self>, node: Active) -> Result<(), String> {
        let profile = node.agent.as_ref().and_then(profile::profile_of);
        let (Some(agent), Some(provider)) = (node.agent.clone(), provider_of(&node)) else {
            return Err(format!(
                "노드 {}는 계정이 있는 에이전트가 아니다",
                node.id.0
            ));
        };
        let settings = self.settings.get().map_err(|e| e.to_string())?;
        let target = settings.account(provider).cloned();
        let new_root = self
            .accounts
            .config_dir(provider, target.as_ref())
            .or_else(|| default_config_dir(provider))
            .ok_or("홈 폴더를 찾지 못했다")?;
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let cwd = Path::new(&place.worktree);
        let resume = match profile {
            Some(Profile::Codex) => self.copy_codex_thread(node.id, &new_root).await?,
            _ => {
                let session = copy_claude_session(node.transcript.as_deref(), &new_root)?;
                // 시스템 기본 폴더는 고치지 않는다(「저장」). 그때 사람이 새 확인 창에 답한다.
                if let Some(to) = self.accounts.config_dir(provider, target.as_ref()) {
                    let from = self.accounts.config_dir(provider, node.account.as_ref());
                    accounts::carry_claude_consent(from.as_deref(), &to, cwd)
                        .map_err(|e| format!("{}: {e}", to.display()))?;
                }
                session
            }
        };
        let brief = match self.switch.limited.lock().unwrap().contains(&node.id) {
            true => CONTINUE_PROMPT.to_owned(),
            false => String::new(),
        };
        let start = Start::Agent {
            agent: agent.clone(),
            brief,
            choice: ModelChoice {
                model: node.model.model.clone(),
                effort: node.model.effort.clone(),
            },
            resume: Some(resume.clone()),
        };
        let position = match self.graph.relations(node.id) {
            Ok(r) if r.parent.is_some() => Position::Child,
            Ok(_) => Position::Root,
            Err(e) => return Err(e.to_string()),
        };
        let prepared = self
            .prepare(&start, cwd, position)
            .await
            .map_err(|e| format!("{e:?}"))?;
        if let Some(server) = &prepared.codex {
            let bypass = !settings.bypass_off.contains(&agent);
            let call = |method, params| server.call(method, params);
            if let Err(e) = codex::resume(call, &resume, bypass).await {
                prepared.discarder()();
                return Err(e);
            }
            server.follow_thread(resume).await;
        }
        let discard = prepared.discarder();
        // 옛 프로세스가 끝난 뒤 띄운다. 옛 PTY 호스트가 끝나며 지우는 주소 파일을 새 호스트가 함께 쓴다.
        let (done, exited) = oneshot::channel();
        self.switch.exiting.lock().unwrap().insert(node.id, done);
        // 옛 CLI의 쉬는 대기를 끝낸다. 새 CLI가 세션을 시작하면 다시 연다.
        self.idle.end(node.id);
        self.host.kill(node.id);
        if tokio::time::timeout(EXIT_WAIT, exited).await.is_err() {
            self.switch.exiting.lock().unwrap().remove(&node.id);
            discard();
            return Err(format!("노드 {}의 옛 프로세스가 끝나지 않았다", node.id.0));
        }
        let launched = match self.launch(node.id, prepared) {
            Ok(launched) => launched,
            Err(e) => {
                discard();
                // 옛 프로세스는 이미 끝났다. 화면이 실행 중으로 남지 않게 한다.
                let _ = self.graph.lost(node.id);
                return Err(e);
            }
        };
        self.switch.limited.lock().unwrap().remove(&node.id);
        self.graph
            .relaunched(node.id, &launched)
            .map_err(|e| e.to_string())
    }

    /// Codex 노드 스레드의 기록 파일을 새 `CODEX_HOME`에 복사하고 스레드 id를 돌려준다. 스레드나 기록 파일이 없으면 실패다.
    async fn copy_codex_thread(&self, node: NodeId, new_root: &Path) -> Result<String, String> {
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
        copy_session(Path::new(&rollout), CODEX_ROLLOUT_DEPTH, new_root)?;
        Ok(thread)
    }

    /// 자동 전환이 켜져 있으면 같은 공급자의 다른 계정 사용량을 그 자리에서 읽고, 남은 계정으로 바꾼다.
    async fn auto_switch(self: &Arc<Self>, provider: Provider) {
        let Ok(mut settings) = self.settings.get() else {
            return;
        };
        if !settings.auto_switch || self.cooling_down(provider) {
            return;
        }
        let current = settings.account(provider).cloned();
        let mut ids = vec![None];
        ids.extend(
            self.accounts
                .list(provider)
                .unwrap_or_default()
                .into_iter()
                .map(Some),
        );
        ids.retain(|id| *id != current);
        let probes = join_all(ids.iter().map(|id| self.probe(provider, id.clone()))).await;
        let candidates: Vec<_> = ids.into_iter().zip(probes).collect();
        self.switch
            .last_auto
            .lock()
            .unwrap()
            .insert(provider, Instant::now());
        let Some(to) = pick_target(&candidates) else {
            let resets_at = earliest_reset(candidates.iter().map(|(_, l)| l));
            if let Err(e) = self.graph.record(&Entry::NoAccountLeft {
                provider,
                resets_at,
            }) {
                eprintln!("loom: 「남은 계정 없음」을 남기지 못했다: {e}");
            }
            return;
        };
        settings.set_account(provider, to);
        if let Err(e) = self.settings.put(&settings) {
            eprintln!("loom: 자동 전환한 계정을 저장하지 못했다: {e}");
            return;
        }
        if let Err(e) = self.switch_nodes(provider, SwitchCause::RateLimit).await {
            eprintln!("loom: 자동 전환으로 노드를 옮기지 못했다: {e:?}");
        }
    }

    fn cooling_down(&self, provider: Provider) -> bool {
        self.switch
            .last_auto
            .lock()
            .unwrap()
            .get(&provider)
            .is_some_and(|at| at.elapsed() < COOLDOWN)
    }

    async fn probe(&self, provider: Provider, id: Option<AccountId>) -> Limits {
        let dir = self.accounts.config_dir(provider, id.as_ref());
        tokio::task::spawn_blocking(move || usage::probe(provider, dir.as_deref()).limits)
            .await
            .unwrap_or(Limits::NeedsLogin)
    }
}

fn provider_of(node: &Active) -> Option<Provider> {
    node.agent
        .as_ref()
        .and_then(profile::profile_of)
        .and_then(Profile::provider)
}

/// 시스템 기본 로그인 폴더. CLI가 환경 변수 없이 쓰는 폴더다.
fn default_config_dir(provider: Provider) -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    Some(home.join(match provider {
        Provider::Claude => ".claude",
        Provider::Codex => ".codex",
    }))
}

/// Claude 세션 기록과 그 곁 폴더(`<세션>/`, 하위 에이전트 기록)를 새 로그인 폴더에 복사하고 세션 id를 돌려준다.
/// 기록 파일이 없으면 실패다. 그 노드는 옮기지 않는다. 세션 id는 기록 파일 이름이다.
fn copy_claude_session(transcript: Option<&str>, new_root: &Path) -> Result<String, String> {
    let file = transcript
        .map(Path::new)
        .filter(|f| f.is_file())
        .ok_or("이어 갈 Claude 세션 기록이 없다")?;
    let session = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or("세션 기록 파일 이름이 없다")?;
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
        .and_then(|()| std::fs::copy(file, &to));
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

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// 가장 많이 나온 계정. 같은 수면 먼저 나온 계정이다. 비었거나 시스템 기본이 가장 많으면 없다.
fn most_used(accounts: impl Iterator<Item = Option<AccountId>>) -> Option<AccountId> {
    let accounts: Vec<_> = accounts.collect();
    let count = |a: &Option<AccountId>| accounts.iter().filter(|b| *b == a).count();
    let mut best: Option<&Option<AccountId>> = None;
    for a in &accounts {
        if best.is_none_or(|b| count(a) > count(b)) {
            best = Some(a);
        }
    }
    best.cloned().flatten()
}

/// 사용량을 읽은 계정 가운데 가장 많이 쓴 창이 가장 적은 계정. 그 값이 `EXHAUSTED_AT` 미만이어야 한다.
/// 읽지 못한 계정은 건너뛴다. 같은 값이면 앞 계정(시스템 기본, 만든 순서)이다.
fn pick_target(candidates: &[(Option<AccountId>, Limits)]) -> Option<Option<AccountId>> {
    candidates
        .iter()
        .filter_map(|(id, limits)| match limits {
            Limits::Ready {
                session, weekly, ..
            } => {
                let used = [session, weekly]
                    .into_iter()
                    .flatten()
                    .map(|w| w.used)
                    .max()
                    .unwrap_or(0);
                (used < EXHAUSTED_AT).then_some((id, used))
            }
            _ => None,
        })
        .min_by_key(|(_, used)| *used)
        .map(|(id, _)| id.clone())
}

/// 계정들의 창 가운데 가장 빠른 초기화 시각.
fn earliest_reset<'a>(limits: impl Iterator<Item = &'a Limits>) -> Option<i64> {
    limits
        .filter_map(|l| match l {
            Limits::Ready {
                session, weekly, ..
            } => Some([session, weekly]),
            _ => None,
        })
        .flatten()
        .flatten()
        .filter_map(|w: &Window| w.resets_at)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(session: u8, weekly: u8) -> Limits {
        Limits::Ready {
            session: Some(Window {
                used: session,
                resets_at: Some(100 + i64::from(session)),
            }),
            weekly: Some(Window {
                used: weekly,
                resets_at: Some(1000),
            }),
            models: Vec::new(),
        }
    }

    fn id(n: u8) -> Option<AccountId> {
        serde_json::from_value(serde_json::json!(format!("{n:016x}"))).unwrap()
    }

    #[test]
    fn target_is_the_least_used_readable_account_with_quota_left() {
        let candidates = vec![
            (None, ready(50, 60)),
            (id(1), Limits::NeedsLogin),
            (id(2), ready(10, 40)),
            (id(3), ready(10, 40)),
            (id(4), ready(0, 100)),
        ];
        assert_eq!(pick_target(&candidates), Some(id(2)));
        let nearly = vec![(None, ready(100, 0)), (id(1), ready(99, 99))];
        assert_eq!(pick_target(&nearly), Some(id(1)));
        let full = vec![(None, ready(100, 0)), (id(1), Limits::NeedsLogin)];
        assert_eq!(pick_target(&full), None);
        assert_eq!(earliest_reset(full.iter().map(|(_, l)| l)), Some(200));
    }

    #[test]
    fn from_is_the_account_most_nodes_had() {
        assert_eq!(most_used([id(1), None, None].into_iter()), None);
        assert_eq!(most_used([id(1), id(2), id(2)].into_iter()), id(2));
        assert_eq!(most_used([id(2), id(1)].into_iter()), id(2));
        assert_eq!(most_used(std::iter::empty()), None);
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

    #[test]
    fn claude_session_copies_the_transcript_and_its_side_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old/projects/-w");
        std::fs::create_dir_all(old.join("s1/subagents")).unwrap();
        std::fs::write(old.join("s1.jsonl"), "x").unwrap();
        std::fs::write(old.join("s1/subagents/a.jsonl"), "y").unwrap();
        let new = tmp.path().join("new");
        let transcript = old.join("s1.jsonl");
        assert_eq!(
            copy_claude_session(transcript.to_str(), &new).unwrap(),
            "s1"
        );
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
        assert!(copy_claude_session(missing.to_str(), &new).is_err());
        assert!(!new.exists());
    }
}
