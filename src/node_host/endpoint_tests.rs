use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::routing::post;
use portable_pty::CommandBuilder;
use tokio::sync::Notify;

use super::*;
use crate::graph::NodeId;
use crate::tunnel;

const NODE: NodeId = NodeId(1);
const HELD: Duration = Duration::from_millis(300);

struct Daemon {
    runtime: tokio::runtime::Runtime,
    host: NodeHost<NodeId>,
    waits: Arc<Notify>,
}

impl Daemon {
    fn start(hosts: &Path) -> Daemon {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let waits = Arc::new(Notify::new());
        let (links, mut linked) = tokio::sync::mpsc::unbounded_channel();
        let (doors, door) = tunnel::Door::<NodeId>::new();
        let router = axum::Router::new()
            .route(
                "/echo",
                post(
                    |ConnectInfo(node): ConnectInfo<NodeId>, body: String| async move {
                        format!("{body} from {}", node.0)
                    },
                ),
            )
            .route(
                "/wait",
                post({
                    let waits = waits.clone();
                    move || async move {
                        waits.notify_one();
                        std::future::pending::<()>().await
                    }
                }),
            );
        runtime.spawn(async move {
            while let Some((node, link)) = linked.recv().await {
                if let Linked::Streams { link, .. } = link {
                    tunnel::carry(link, move || node, doors.clone()).unwrap();
                }
            }
        });
        runtime.spawn(async move {
            axum::serve(door, router.into_make_service_with_connect_info::<NodeId>()).await
        });
        let host = NodeHost::new(hosts.to_path_buf()).with_links(links);
        host.resume(NODE, |_| {}).unwrap();
        Daemon {
            runtime,
            host,
            waits,
        }
    }
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, tunnel::Door<NodeId>>>
    for NodeId
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, tunnel::Door<NodeId>>) -> NodeId {
        *stream.remote_addr()
    }
}

#[cfg(unix)]
fn endpoint(dir: &Path, name: &str) -> OsString {
    dir.join("run").join(format!("{name}.sock")).into()
}

#[cfg(windows)]
fn endpoint(_: &Path, name: &str) -> OsString {
    format!(r"\\.\pipe\loom-test-{}-{name}", std::process::id()).into()
}

#[cfg(unix)]
fn waiting_command() -> CommandBuilder {
    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", "printf ready; read line"]);
    cmd
}

#[cfg(windows)]
fn waiting_command() -> CommandBuilder {
    let mut cmd = CommandBuilder::new("cmd");
    cmd.args(["/c", "ping -n 60 127.0.0.1 >nul"]);
    cmd
}

#[cfg(unix)]
fn connect(at: &OsString) -> impl Read + Write {
    std::os::unix::net::UnixStream::connect(at).unwrap()
}

#[cfg(windows)]
fn connect(at: &OsString) -> impl Read + Write {
    tunnel::open_pipe(
        Path::new(at),
        std::fs::OpenOptions::new().read(true).write(true),
    )
    .unwrap()
}

fn ask(at: &OsString, path: &str, body: &str) -> String {
    let mut conn = connect(at);
    write!(
        conn,
        "POST {path} HTTP/1.1\r\nhost: loom\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut got = Vec::new();
    let _ = conn.read_to_end(&mut got);
    String::from_utf8_lossy(&got).into_owned()
}

fn asking(
    at: &OsString,
    path: &'static str,
    body: &'static str,
) -> std::thread::JoinHandle<String> {
    let at = at.clone();
    std::thread::spawn(move || ask(&at, path, body))
}

#[test]
fn a_request_at_the_endpoint_waits_for_the_daemon_and_one_open_at_its_exit_closes() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    let at = endpoint(dir.path(), "held");
    let started = host::start(waiting_command(), None, Some(&at)).unwrap();
    std::fs::create_dir_all(&hosts).unwrap();
    std::fs::write(hosts.join("1"), started.address.to_string()).unwrap();
    std::thread::spawn(move || started.wait());

    let early = asking(&at, "/echo", "early");
    std::thread::sleep(HELD);
    assert!(!early.is_finished(), "no daemon yet: the endpoint holds it");
    let daemon = Daemon::start(&hosts);
    let answer = early.join().unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
    assert!(answer.ends_with("early from 1"), "{answer}");

    let cut = asking(&at, "/wait", "");
    daemon.runtime.block_on(daemon.waits.notified());
    drop(daemon);
    assert_eq!(
        cut.join().unwrap(),
        "",
        "the daemon left: the open request closes"
    );

    let again = asking(&at, "/echo", "again");
    std::thread::sleep(HELD);
    assert!(!again.is_finished());
    let daemon = Daemon::start(&hosts);
    assert!(again.join().unwrap().ends_with("again from 1"));
    daemon.host.kill(NODE);
}

#[test]
fn an_old_pty_host_attaches_without_a_stream_link() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    std::fs::write(hosts.join("1"), listener.local_addr().unwrap().to_string()).unwrap();
    let old = std::thread::spawn(move || {
        let (mut daemon, _) = listener.accept().unwrap();
        assert!(matches!(wire::read(&mut daemon), Ok(Frame::Hello { .. })));
        wire::write(
            &mut daemon,
            &Frame::Hello {
                version: 3,
                pid: Some(9),
            },
        )
        .unwrap();
        wire::write(&mut daemon, &Frame::Resize { cols: 80, rows: 24 }).unwrap();
        listener.set_nonblocking(true).unwrap();
        std::thread::sleep(HELD);
        (daemon, listener.accept().is_ok())
    });
    let (links, mut linked) = tokio::sync::mpsc::unbounded_channel();
    let host = NodeHost::new(hosts).with_links(links);
    host.resume(NODE, |_| {}).unwrap();
    assert_eq!(host.pid(NODE), Some(9));
    let (_daemon, second) = old.join().unwrap();
    assert!(!second, "a version 3 host gets no stream link");
    assert!(matches!(linked.try_recv(), Ok((NODE, Linked::Terminal))));
}

#[test]
fn a_host_that_does_not_greet_the_stream_link_is_killed_and_not_attached() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    std::fs::write(hosts.join("1"), listener.local_addr().unwrap().to_string()).unwrap();
    let silent = std::thread::spawn(move || {
        let (mut terminal, _) = listener.accept().unwrap();
        assert!(matches!(wire::read(&mut terminal), Ok(Frame::Hello { .. })));
        wire::write(
            &mut terminal,
            &Frame::Hello {
                version: wire::STREAMS_SINCE,
                pid: Some(9),
            },
        )
        .unwrap();
        let (mut streams, _) = listener.accept().unwrap();
        assert!(matches!(
            wire::read(&mut streams),
            Ok(Frame::Streams { .. })
        ));
        (wire::read(&mut terminal).unwrap(), streams)
    });
    let (links, mut linked) = tokio::sync::mpsc::unbounded_channel();
    let host = NodeHost::new(hosts.clone()).with_links(links);
    let got = host.resume(NODE, |_| {});
    assert!(
        matches!(&got, Err(Unattached::Silent(e)) if e.starts_with("node endpoint link: ")),
        "{got:?}"
    );
    let (sent, _streams) = silent.join().unwrap();
    assert_eq!(sent, Frame::Kill);
    assert!(linked.try_recv().is_err());
    assert!(!hosts.join("1").exists());
    assert_eq!(host.pid(NODE), None);
}

#[test]
fn a_host_reports_an_endpoint_it_cannot_bind_to_the_daemon() {
    use super::endpoint::Endpoint;
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    let at = endpoint(dir.path(), "taken");
    let _other = Endpoint::open(&at, drop).unwrap();
    let started = host::start(waiting_command(), None, Some(&at)).unwrap();
    std::fs::create_dir_all(&hosts).unwrap();
    std::fs::write(hosts.join("1"), started.address.to_string()).unwrap();
    let stop = started.stopper();
    std::thread::spawn(move || started.wait());
    let (links, mut linked) = tokio::sync::mpsc::unbounded_channel();
    let host = NodeHost::new(hosts).with_links(links);
    host.resume(NODE, |_| {}).unwrap();
    let failed = loop {
        if let (NODE, Linked::EndpointFailed(error)) = linked.blocking_recv().unwrap() {
            break error;
        }
    };
    assert!(failed.starts_with(&*at.to_string_lossy()), "{failed}");
    stop();
}

#[cfg(unix)]
#[test]
fn the_outer_host_carries_the_terminal_and_the_endpoint_of_the_inner_host() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    let at = endpoint(dir.path(), "inner");
    let started = host::start(waiting_command(), None, Some(&at)).unwrap();
    let inner = started.address;
    std::thread::spawn(move || started.wait());
    let forwarding = tokio::runtime::Runtime::new().unwrap();
    let outer = forwarding.block_on(async move {
        let (a, b) = tokio::io::duplex(1 << 16);
        let (from_outer, to_outer) = tokio::io::split(a);
        let (from_inner, to_inner) = tokio::io::split(b);
        tokio::spawn(inner::carry_in(
            inner,
            from_outer,
            to_outer,
            std::future::pending(),
        ));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let outer = listener.local_addr().unwrap();
        tokio::spawn(inner::carry_out(listener, from_inner, to_inner));
        outer
    });
    std::fs::create_dir_all(&hosts).unwrap();
    std::fs::write(hosts.join("1"), outer.to_string()).unwrap();

    let daemon = Daemon::start(&hosts);
    let mut attached = daemon.host.attach(NODE).unwrap();
    let mut seen = String::from_utf8_lossy(&attached.backlog).into_owned();
    while !seen.contains("ready") {
        let chunk = attached.output.blocking_recv().unwrap();
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(ask(&at, "/echo", "inside").ends_with("inside from 1"));
    daemon.host.kill(NODE);
}

#[cfg(unix)]
#[test]
fn an_endpoint_takes_a_stale_socket_but_never_a_live_one_or_an_inherited_name() {
    use super::endpoint::Endpoint;
    let dir = tempfile::tempdir().unwrap();
    let live = endpoint(dir.path(), "live");
    let first = Endpoint::open(&live, drop).unwrap();
    assert!(
        Endpoint::open(&live, drop).is_err(),
        "another host holds it"
    );
    drop(first);
    assert!(
        !Path::new(&live).exists(),
        "the host removes its own socket"
    );

    let stale = endpoint(dir.path(), "stale");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    assert!(Endpoint::open(&stale, drop).is_ok());

    let inherited = endpoint(dir.path(), "inherited");
    let mut cmd = waiting_command();
    cmd.env(crate::tunnel::ENDPOINT_ENV, &inherited);
    in_process(&dir.path().join("hosts"), NODE, cmd);
    assert!(
        !Path::new(&inherited).exists(),
        "only the spec gives a host its endpoint"
    );
}

#[test]
fn a_host_counts_the_connections_it_held_for_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    let at = endpoint(dir.path(), "count");
    let counted = |id: NodeId, endpoint: Option<&OsString>| {
        let started =
            host::start(waiting_command(), None, endpoint.map(OsString::as_os_str)).unwrap();
        std::fs::write(hosts.join(id.0.to_string()), started.address.to_string()).unwrap();
        std::thread::spawn(move || started.wait());
    };
    counted(NODE, Some(&at));
    counted(NodeId(2), None);
    let _held = [asking(&at, "/echo", "a"), asking(&at, "/echo", "b")];
    std::thread::sleep(HELD);
    let (links, mut linked) = tokio::sync::mpsc::unbounded_channel();
    let host = NodeHost::new(hosts).with_links(links);
    let mut held = |id: NodeId| {
        host.resume(id, |_| {}).unwrap();
        match linked.try_recv() {
            Ok((got, Linked::Streams { held, .. })) if got == id => held,
            _ => panic!("no stream link"),
        }
    };
    assert_eq!(held(NODE), Some(2));
    assert_eq!(
        held(NodeId(2)),
        Some(0),
        "a host without an endpoint holds nothing"
    );
    host.kill(NODE);
    host.kill(NodeId(2));
}
