mod agent;
mod arrival;
mod carry;
mod commands;
mod deploy;
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
mod spent;
pub use crate::graph::NewAgent;
pub use relaunch::Relaunched;
mod switch;
mod system;
mod turn_end;
mod usage_cache;
mod verbs;
mod waits;

pub use commands::{
    AdoptRequest, AnswerRequest, AnsweredRequest, AskRequest, AskResult, Asked, DetachRequest,
    DoneRequest, GuardRequest, NoteRequest, RelaunchRequest, RemoveRequest, Reported, ShowRequest,
    SpawnRequest, SpawnRun, WaitRequest,
};
use origin::ViaRemote;
use runtime::Prepared;

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::IntoFuture;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
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
use ts_rs::TS;

use crate::accounts::{AccountId, Site};
use crate::commit_graph::{self, Commit};
use crate::db;
use crate::devices::Devices;
use crate::graph::{
    AgentEvent, CardView, EventLog, Graph, GraphEvent, IdleEnd, Kept, Launched, Listed, NewChild,
    NodeId, NodeView, PARENT, Place, Relations, Report, Sent, Via, WaitSource,
};
use crate::launch;
use crate::node_host::{self, Exit, NodeHost};
use crate::open;
use crate::paste;
use crate::profile::adapter::Chosen;
use crate::profile::{self, Profile, Start};
use crate::protocol::{
    AckRequest, Delivery, EndedTurn, IMAGE_EXTENSIONS, IdleReply, IdleRequest, Kind, LaunchError,
    MessageId, MessageKind, Model, Prompt, Question, REVIEW_RULES, Refusal, Strokes, TakeRequest,
    Taken, TreeLimits, WaitDoc, WaitId, entered,
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
    Attempt, agent_adopt, agent_answer, agent_answered, agent_ask, agent_detach, agent_done,
    agent_note, agent_remove, agent_set_role, agent_spawn, endpoint_spawn, traced,
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
    sessions: sessions::Sessions,
    wakes: sessions::Wakes,
    idle: idle::IdleWaits,
    relaunch_exits: tokio::sync::Notify,
    sleeps: sleep::Sleeps,
    switch: switch::Switches,
    usage: usage_cache::UsageCache,
    repo_list: one_flight::OneFlight<Result<Vec<RepoView>, String>>,
    turn_ends: turn_end::TurnEnds,
    merge_queue: tokio::sync::Notify,
    ptys_changed: tokio::sync::Notify,
    plugin: skills::Plugin,
    shim: PathBuf,
    linux_loom: Option<PathBuf>,
    distros: Option<Arc<crate::distro::Distros>>,
    run_dir: PathBuf,
    versions: HashMap<Profile, String>,
    node_ip: Option<IpAddr>,
    url: String,
    spawn_url: String,
    update: Arc<Updater>,
    devices: Arc<Devices>,
    port: u16,
    door: lan::Door,
    passes: passes::Passes,
    choices: waits::Choices,
    doc_titles: user_doc::Titles,
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

pub async fn serve(listen: SocketAddr, ready_to: Option<SocketAddr>) -> std::io::Result<()> {
    let claim = launch::Claim::take()?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let listen = listener.local_addr()?;
    let node_door = match tokio::net::TcpListener::bind((NODE_IP, listen.port())).await {
        Ok(door) => Some(door),
        Err(_) => tokio::net::TcpListener::bind((NODE_IP, 0)).await.ok(),
    };
    let db_path = db::default_path()?;
    let node_door_addr = node_door.as_ref().map(|d| d.local_addr()).transpose()?;
    let (doors, endpoint_door) = tunnel::Door::<arrival::Caller>::new();
    let (links, linked) = tokio::sync::mpsc::unbounded_channel();
    let app = Arc::new(App::new(
        db::open(&db_path)?,
        &db_path,
        listen,
        node_door_addr,
        links,
    )?);
    tokio::spawn(app.clone().carry_links(linked, doors));
    tokio::task::spawn_blocking({
        let app = app.clone();
        move || user_doc::docs(&app.doc_titles)
    });
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
    claim.announce(&node_url(listen), ready_to)?;
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
            if let Err(e) = app.finish_stops() {
                eprintln!("loom: 끝내던 다시 띄우기를 보지 못했다: {e}");
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
        .merge(commands::endpoint_only_routes())
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
        let devices = Arc::new(Devices::new(database.clone(), {
            let graph = graph.clone();
            move || graph.publish_view_changed()
        }));
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
        let host = NodeHost::new(pty_dir.join("nodes.screens")).with_links(links);
        let turn_ends = turn_end::TurnEnds::new(graph.clone());
        let app = App {
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
            choices: waits::Choices::default(),
            doc_titles: user_doc::Titles::default(),
            spots: Spots::new(database.clone()),
            database,
            accounts_root: db_path.with_file_name("accounts"),
            logins: settings::Logins::new(pty_dir.join("logins.screens")),
            sessions: sessions::Sessions::default(),
            wakes: sessions::Wakes::default(),
            idle,
            relaunch_exits: tokio::sync::Notify::new(),
            sleeps: sleep::Sleeps::default(),
            switch: switch::Switches::default(),
            usage: usage_cache::UsageCache::default(),
            repo_list: one_flight::OneFlight::default(),
            turn_ends,
            merge_queue: tokio::sync::Notify::new(),
            ptys_changed: tokio::sync::Notify::new(),
            plugin: {
                let plugin =
                    skills::Plugin::install(db_path.parent().expect("DB 파일에는 폴더가 있다"))?;
                for dir in plugin.dirs() {
                    profile::install(&dir)?;
                }
                plugin
            },
            shim,
            linux_loom,
            distros,
            run_dir: db_path.with_file_name("run"),
            versions: profile::versions(),
            node_ip: node_door.map(|_| NODE_IP.into()),
            url: match node_door {
                Some(door) => format!("http://{door}"),
                None => node_url(listen),
            },
            spawn_url: node_url(listen),
            update,
        };
        app.adopt_address_files(&pty_dir)
            .map_err(std::io::Error::other)?;
        Ok(app)
    }
}

fn internal(e: impl std::fmt::Display) -> Problem {
    Problem::Internal(e.to_string())
}
