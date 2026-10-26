mod agent;
mod arrival;
mod carry;
mod commands;
pub mod gate;
mod idle;
mod lan;
mod nodes;
mod one_flight;
mod origin;
mod passes;
mod phone;
mod places;
mod pulse;
mod queue;
mod reclaim;
mod relaunch;
mod repos;
mod runtime;
mod sessions;
mod settings;
mod sleep;
pub use crate::graph::NewAgent;
pub use relaunch::Relaunched;
mod switch;
mod system;
mod turn_end;
mod usage_cache;
#[cfg(test)]
mod verb_traces;
mod verbs;
mod waits;

pub use commands::{
    AckRequest, AdoptRequest, AnswerRequest, AnsweredRequest, AskRequest, AskResult, Asked,
    DoneRequest, EndedTurn, GuardRequest, HookWaitRequest, IdleReply, IdleRequest, NoteRequest,
    RelaunchRequest, RemoveRequest, Reported, SaidRequest, SpawnRequest, SpawnRun, TakeRequest,
    Taken, WaitRequest,
};
use origin::ViaRemote;
use runtime::Prepared;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::future::IntoFuture;
use std::hash::Hash;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use ts_rs::TS;

use crate::accounts::{AccountId, Site};
use crate::commit_graph::{self, Commit};
use crate::db;
use crate::devices::Devices;
use crate::graph::{
    AgentEvent, CardView, EventLog, Graph, GraphEvent, IdleEnd, Kept, Launched, Listed, NewChild,
    NodeId, PARENT, Place, Relations, Report, Sent, Via, WaitSource,
};
use crate::launch;
use crate::node_host::{self, Exit, NodeHost};
use crate::open;
use crate::paste;
use crate::profile::codex::server::CodexServer;
use crate::profile::codex::{self, Status};
use crate::profile::cursor;
use crate::profile::omp;
use crate::profile::opencode2;
use crate::profile::{self, Profile, Remote, Start, VersionMismatch};
use crate::protocol::{
    Delivery, IMAGE_EXTENSIONS, Kind, LaunchError, MessageId, MessageKind, Model, Prompt, Question,
    Refusal, ServerLink, TreeLimits, WaitDoc, WaitId, WaitKind, entered,
};
use crate::roles::{RoleProblem, Roles, SetRole};
use crate::settings::Settings;
use crate::skills::{self, Launch, Position};
use crate::spots::{Arranged, CardSize, Layout, Placed, Spots};
use crate::task;
use crate::tunnel;
use crate::update::{UpdateView, Updater};
use crate::user_doc;
use crate::worktree::{self, FolderList, RepoId, RepoProblem, RepoView, Repos, Worktree};
use crate::wsl;
use verbs::{
    Attempt, agent_adopt, agent_answer, agent_answered, agent_ask, agent_done, agent_note,
    agent_remove, agent_set_role, agent_spawn, traced,
};

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

struct App {
    database: db::Database,
    repos: Repos,
    host: NodeHost<NodeId>,
    graph: Arc<Graph>,
    settings: Settings,
    spots: Spots,
    accounts_root: PathBuf,
    logins: settings::Logins,
    codex: Mutex<HashMap<NodeId, Arc<CodexServer>>>,
    opencode2: Mutex<HashMap<NodeId, Arc<opencode2::Node>>>,
    sessions: sessions::Sessions,
    first_inputs: sessions::FirstInputs,
    setups: Mutex<HashSet<NodeId>>,
    idle: idle::IdleWaits,
    relaunches: relaunch::Relaunches,
    sleeps: sleep::Sleeps,
    switch: switch::Switches,
    usage: usage_cache::UsageCache,
    repo_list: one_flight::OneFlight<Result<Vec<RepoView>, String>>,
    turn_ends: turn_end::TurnEnds,
    merge_queue: tokio::sync::Notify,
    plugin: skills::Plugin,
    shim: PathBuf,
    linux_loom: Option<PathBuf>,
    distros: Option<Arc<crate::distro::Distros>>,
    run_dir: PathBuf,
    homes: wsl::Homes,
    version_warnings: HashMap<Profile, VersionMismatch>,
    codex_config: Vec<String>,
    url: String,
    spawn_url: String,
    update: Arc<Updater>,
    devices: Arc<Devices>,
    port: u16,
    door: lan::Door,
    passes: passes::Passes,
}

/// 화면 경로가 실패하면 보내는 몸통(JSON). 화면이 고른 언어로 쓴다.
#[derive(Debug, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum Problem {
    Refused(Refusal),
    Launch(LaunchError),
    Repo(RepoProblem),
    /// 없는 로그인 창이다.
    NoLogin,
    /// DB·파일 오류처럼 요청을 고쳐도 낫지 않는 실패. 값은 원인 글이다.
    Internal(String),
}

impl Problem {
    fn status(&self) -> StatusCode {
        match self {
            Problem::Refused(Refusal::Db(_)) | Problem::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Problem::Refused(_) | Problem::Launch(_) | Problem::Repo(_) => StatusCode::BAD_REQUEST,
            Problem::NoLogin => StatusCode::NOT_FOUND,
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        (self.status(), Json(self)).into_response()
    }
}

impl From<Refusal> for Problem {
    fn from(e: Refusal) -> Self {
        Problem::Refused(e)
    }
}

impl From<RepoProblem> for Problem {
    fn from(e: RepoProblem) -> Self {
        Problem::Repo(e)
    }
}

impl From<LaunchError> for Problem {
    fn from(e: LaunchError) -> Self {
        Problem::Launch(e)
    }
}

type AgentError = (StatusCode, String);

fn for_agent(e: impl Into<Problem>) -> AgentError {
    let problem = e.into();
    let text = match &problem {
        Problem::Refused(e) => e.to_string(),
        Problem::Launch(e) => e.to_string(),
        Problem::Repo(e) => e.to_string(),
        Problem::NoLogin => "No such login window.".into(),
        Problem::Internal(e) => e.clone(),
    };
    (problem.status(), text)
}

const SCREEN_QUIET: Duration = Duration::from_millis(500);

const NODE_IP: Ipv4Addr = Ipv4Addr::new(127, 76, 77, 1);

pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    let claim = launch::Claim::take()?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let listen = listener.local_addr()?;
    let node_door = match tokio::net::TcpListener::bind((NODE_IP, listen.port())).await {
        Ok(door) => Some(door),
        Err(_) => tokio::net::TcpListener::bind((NODE_IP, 0)).await.ok(),
    };
    let codex_version = codex::installed_version();
    let db_path = db::default_path()?;
    let node_door_addr = node_door.as_ref().map(|d| d.local_addr()).transpose()?;
    let (doors, endpoint_door) = tunnel::Door::<arrival::Caller>::new();
    let (links, linked) = tokio::sync::mpsc::unbounded_channel();
    let app = Arc::new(App::new(
        db::open(&db_path)?,
        &db_path,
        listen,
        node_door_addr,
        codex_version.as_deref(),
        links,
    )?);
    tokio::spawn(app.clone().carry_links(linked, doors));
    match app
        .repos
        .add(&std::env::current_dir()?.to_string_lossy())
        .await
    {
        Ok(_) | Err(RepoProblem::NotGit(_) | RepoProblem::NotFound(_)) => {}
        Err(problem) => return Err(std::io::Error::other(problem.to_string())),
    }
    if let Some((repo, path)) = app
        .repos
        .list()
        .map_err(std::io::Error::other)?
        .into_iter()
        .next()
    {
        app.graph
            .settle(&Place {
                repo,
                worktree: path.to_string_lossy().into_owned(),
            })
            .map_err(std::io::Error::other)?;
    }
    app.start_nodes();
    let node_routes = commands::node_routes();
    let router = node_routes
        .clone()
        .merge(commands::routes())
        .merge(repos::routes())
        .merge(nodes::routes())
        .merge(waits::routes())
        .merge(system::routes())
        .merge(settings::routes())
        .merge(places::routes())
        .merge(switch::routes())
        .merge(phone::routes())
        .merge(passes::routes())
        .merge(relaunch::routes())
        .merge(carry::routes())
        .fallback(system::asset)
        .layer(axum::middleware::from_fn_with_state(
            app.devices.clone(),
            origin::human_site,
        ))
        .with_state(app.clone());
    app.door.set_router(router.clone());
    if !app
        .devices
        .list()
        .map_err(std::io::Error::other)?
        .is_empty()
    {
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(e) = app.door.open().await {
                eprintln!("loom: phone door: {e}");
            }
        });
    }
    claim.announce(&node_url(listen))?;
    tokio::spawn(app.update.clone().run());
    tokio::spawn(queue::run(app.clone()));
    println!("loom: http://{listen}");
    let main = axum::serve(listener, router).into_future();
    let endpoint = axum::serve(endpoint_door, endpoint_routes(&app)).into_future();
    let node_routes = node_routes
        .layer(axum::middleware::from_fn(origin::same_site))
        .with_state(app);
    match node_door {
        Some(door) => {
            let node = axum::serve(door, node_routes).into_future();
            tokio::try_join!(main, node, endpoint).map(drop)
        }
        None => tokio::try_join!(main, endpoint).map(drop),
    }
}

impl App {
    fn start_nodes(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            if let Err(e) = app.resume().await {
                eprintln!("loom: 노드에 다시 붙지 못했다: {e}");
            }
            if let Err(e) = app.relaunch_stale() {
                eprintln!("loom: 시작 글이 바뀐 노드를 고르지 못했다: {e}");
            }
            app.watch_sleep();
            app.watch_agents();
        });
    }
}

fn endpoint_routes(
    app: &Arc<App>,
) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, arrival::Caller> {
    commands::node_routes()
        .layer(axum::middleware::from_fn(origin::same_site))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            runtime::from_its_node,
        ))
        .with_state(app.clone())
        .into_make_service_with_connect_info::<arrival::Caller>()
}

fn node_url(listen: SocketAddr) -> String {
    let ip = match listen.ip() {
        ip if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, listen.port()))
}

fn node_path() -> std::io::Result<OsString> {
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let searched = crate::search_path::current();
    let dirs = std::iter::once(exe_dir).chain(std::env::split_paths(&searched));
    std::env::join_paths(dirs).map_err(std::io::Error::other)
}

impl App {
    fn new(
        database: db::Database,
        db_path: &std::path::Path,
        listen: SocketAddr,
        node_door: Option<SocketAddr>,
        codex_version: Option<&str>,
        links: node_host::Links<NodeId>,
    ) -> std::io::Result<App> {
        let repos = Repos::new(database.clone());
        let graph = Arc::new(
            Graph::open(database.clone(), profile::linked).map_err(std::io::Error::other)?,
        );
        let pty_dir = db_path.with_file_name("pty");
        let update = Arc::new(Updater::new(listen, {
            let graph = graph.clone();
            move || graph.publish_view_changed()
        }));
        let devices = Arc::new(Devices::new(database.clone()));
        let idle = idle::IdleWaits::new(graph.clone());
        let shim = db_path.with_file_name("wsl");
        let linux_loom = match cfg!(windows) {
            true => wsl::install_linux(
                &shim,
                &std::env::current_exe()?.with_file_name(wsl::LINUX_LOOM),
            )
            .unwrap_or_else(|e| {
                eprintln!("loom: WSL 노드용 Linux loom을 넣지 못했다: {e}");
                None
            }),
            false => None,
        };
        if let Some(linux) = &linux_loom {
            wsl::prune_linux(linux);
        }
        if cfg!(windows) {
            wsl::write_shim(&shim, &std::env::current_exe()?, linux_loom.as_deref())?;
        }
        let distros = linux_loom.as_ref().map(|loom| {
            Arc::new(crate::distro::Distros::through_wsl(
                pty_dir.join("distros"),
                loom,
            ))
        });
        let host = NodeHost::new(pty_dir.join("nodes")).with_links(links);
        Ok(App {
            repos,
            host: match &distros {
                Some(distros) => host.with_distros(distros.clone()),
                None => host,
            },
            graph,
            settings: Settings::new(database.clone(), profile::check_choice),
            devices: devices.clone(),
            port: listen.port(),
            door: lan::Door::new(
                db_path
                    .parent()
                    .expect("DB 파일에는 폴더가 있다")
                    .to_path_buf(),
                listen.port(),
                devices,
            ),
            passes: passes::Passes::default(),
            spots: Spots::new(database.clone()),
            database,
            accounts_root: db_path.with_file_name("accounts"),
            logins: settings::Logins::new(pty_dir.join("logins")),
            codex: Mutex::default(),
            opencode2: Mutex::default(),
            sessions: sessions::Sessions::default(),
            first_inputs: sessions::FirstInputs::default(),
            setups: Mutex::default(),
            idle,
            relaunches: relaunch::Relaunches::default(),
            sleeps: sleep::Sleeps::default(),
            switch: switch::Switches::default(),
            usage: usage_cache::UsageCache::default(),
            repo_list: one_flight::OneFlight::default(),
            turn_ends: turn_end::TurnEnds::default(),
            merge_queue: tokio::sync::Notify::new(),
            plugin: {
                let plugin =
                    skills::Plugin::install(db_path.parent().expect("DB 파일에는 폴더가 있다"))?;
                for dir in plugin.dirs() {
                    profile::cursor::write_plugin(&dir)?;
                    profile::write_extensions(&dir)?;
                }
                plugin
            },
            shim,
            linux_loom,
            distros,
            run_dir: db_path.with_file_name("run"),
            homes: wsl::Homes::default(),
            version_warnings: profile::version_warnings(codex_version),
            codex_config: match (codex_version, node_door) {
                (Some(version), Some(_)) => codex::sandbox_args(version, NODE_IP.into()),
                _ => Vec::new(),
            },
            url: match node_door {
                Some(door) => format!("http://{door}"),
                None => node_url(listen),
            },
            spawn_url: node_url(listen),
            update,
        })
    }
}

fn internal(e: impl std::fmt::Display) -> Problem {
    Problem::Internal(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problem_is_a_code_and_value() {
        let json = |p: Problem| serde_json::to_value(p).unwrap();
        assert_eq!(
            json(Refusal::NameTaken("a".into()).into()),
            serde_json::json!({"code": "refused", "value": {"code": "nameTaken", "value": "a"}})
        );
        assert_eq!(
            json(
                LaunchError::BriefTooLong {
                    len: 5000,
                    limit: 4096
                }
                .into()
            ),
            serde_json::json!({"code": "launch", "value": {"code": "briefTooLong", "value": {"len": 5000, "limit": 4096}}})
        );
        assert_eq!(
            json(Problem::NoLogin),
            serde_json::json!({"code": "noLogin"})
        );
        assert_eq!(Problem::NoLogin.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            for_agent(Refusal::EmptyName),
            (StatusCode::BAD_REQUEST, "Give a name.".into())
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_daemon_answers_http_while_a_host_resume_blocks() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("loom.db");
        let database = db::open(&db_path).unwrap();
        database
            .lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let app = Arc::new(
            App::new(
                database,
                &db_path,
                "127.0.0.1:0".parse().unwrap(),
                None,
                None,
                tokio::sync::mpsc::unbounded_channel().0,
            )
            .unwrap(),
        );
        let place = Place {
            repo: crate::worktree::RepoId(1),
            worktree: "/repo".into(),
        };
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let hosts = dir.path().join("pty").join("nodes");
        std::fs::create_dir_all(&hosts).unwrap();
        let mut nodes = Vec::new();
        for name in ["a", "b", "c"] {
            let launched = crate::graph::Launched {
                token: name.into(),
                server: None,
                account: None,
                rules: None,
            };
            let id = app
                .graph
                .add_root(&place, None, None, &launched, |_| Ok(()))
                .unwrap();
            std::fs::write(
                hosts.join(id.0.to_string()),
                silent.local_addr().unwrap().to_string(),
            )
            .unwrap();
            nodes.push(id);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let routes = settings::routes().with_state(app.clone());
        tokio::spawn(async move { axum::serve(listener, routes).await });

        let started = std::time::Instant::now();
        app.start_nodes();
        let mut http = tokio::net::TcpStream::connect(address).await.unwrap();
        http.write_all(
            b"GET /api/settings HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        let mut answer = String::new();
        http.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            app.graph.active().unwrap().len(),
            nodes.len(),
            "the hosts still block their resume"
        );
        while !app.graph.active().unwrap().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(30));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_node_removed_during_startup_is_not_attached_after_its_removal() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("loom.db");
        let database = db::open(&db_path).unwrap();
        database
            .lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let app = Arc::new(
            App::new(
                database,
                &db_path,
                "127.0.0.1:0".parse().unwrap(),
                None,
                None,
                tokio::sync::mpsc::unbounded_channel().0,
            )
            .unwrap(),
        );
        let place = Place {
            repo: crate::worktree::RepoId(1),
            worktree: "/repo".into(),
        };
        let mut nodes = Vec::new();
        for name in ["blocked", "removed"] {
            let launched = crate::graph::Launched {
                token: name.into(),
                server: None,
                account: None,
                rules: None,
            };
            nodes.push(
                app.graph
                    .add_root(&place, None, None, &launched, |_| Ok(()))
                    .unwrap(),
            );
        }
        let [blocked, removed] = nodes[..] else {
            unreachable!()
        };
        let hosts = dir.path().join("pty/nodes");
        std::fs::create_dir_all(&hosts).unwrap();
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(
            hosts.join(blocked.0.to_string()),
            silent.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let blocker = std::thread::spawn(move || {
            let (link, _) = silent.accept().unwrap();
            entered.send(()).unwrap();
            gate.recv().unwrap();
            drop(link);
        });
        let mut cmd = portable_pty::CommandBuilder::new("sh");
        cmd.args(["-c", "printf 'still here'; exec cat >/dev/null"]);
        crate::node_host::in_process(&hosts, removed, cmd);
        app.start_nodes();
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        assert!(
            app.host.attach(removed).is_none(),
            "resume is still at the first host"
        );
        app.remove(removed).unwrap();
        assert!(!app.graph.active().unwrap().iter().any(|n| n.id == removed));
        release.send(()).unwrap();
        let resumed = tokio::time::timeout(Duration::from_secs(3), async {
            while app.host.attach(removed).is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        app.host.kill(removed);
        app.host.kill(blocked);
        blocker.join().unwrap();
        assert!(
            !resumed,
            "startup attached a deleted node and kept its process outside the graph"
        );
    }
}
