use std::path::Path;
use std::time::{Duration, Instant};

use super::endpoint_tests::{Daemon, NODE, ask, endpoint};
use super::*;
use crate::graph::NodeId;
use crate::tunnel::ENDPOINT_ENV;

const DISTRO: &str = "Ubuntu";
const LIMIT: Duration = Duration::from_secs(10);

fn start_here(start: distro::Start) -> Result<SocketAddr, String> {
    let (_, cmd, setup) = distro_commands(start);
    let endpoint = cmd.get_env(ENDPOINT_ENV).map(ToOwned::to_owned);
    let started = host::start(cmd, setup, endpoint.as_deref())?;
    let at = started.address;
    std::thread::spawn(move || started.wait());
    Ok(at)
}

#[cfg(unix)]
const WAITING: [&str; 3] = ["sh", "-c", "printf ready; read line"];

#[cfg(windows)]
const WAITING: [&str; 3] = ["cmd", "/c", "echo ready & ping -n 60 127.0.0.1 >nul"];

fn waiting(dir: &Path, endpoint: &std::ffi::OsStr) -> distro::Start {
    distro::Start {
        program: "loom".into(),
        command: distro::Command {
            argv: WAITING.map(str::to_owned).to_vec(),
            cwd: dir.display().to_string(),
            env: vec![(ENDPOINT_ENV.into(), endpoint.to_string_lossy().into_owned())],
        },
        setup: None,
    }
}

fn shown(daemon: &Daemon, id: NodeId, text: &str) {
    let mut attached = daemon.host.attach(id).unwrap();
    let mut seen = String::from_utf8_lossy(&attached.backlog).into_owned();
    while !seen.contains(text) {
        let chunk = attached.output.blocking_recv().unwrap();
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
}

fn until(done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < LIMIT, "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_distro_host_carries_the_terminal_and_the_endpoint_of_a_node_it_started() {
    let dir = tempfile::tempdir().unwrap();
    let (distros, local) = distro::local(dir.path().join("distros"), start_here);
    let daemon = Daemon::new(NodeHost::new(dir.path().join("hosts")).with_distros(distros));
    let at = endpoint(dir.path(), "carried");
    daemon
        .host
        .spawn_in(NODE, DISTRO, waiting(dir.path(), &at), |_| {})
        .unwrap();
    shown(&daemon, NODE, "ready");
    assert!(ask(&at, "/echo", "inside").ends_with("inside from 1"));
    assert_eq!(local.launched(), 1);
    daemon.host.kill(NODE);
    local.end_all();
}

#[test]
fn spawns_wakes_and_a_daemon_restart_of_many_nodes_launch_one_distro_host() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    let (distros, first) = distro::local(dir.path().join("distros"), start_here);
    let daemon = Daemon::new(NodeHost::new(hosts.clone()).with_distros(distros));
    for id in 1..=4 {
        let at = endpoint(dir.path(), &format!("n{id}"));
        daemon
            .host
            .spawn_in(NodeId(id), DISTRO, waiting(dir.path(), &at), |_| {})
            .unwrap();
    }
    assert_eq!(first.launched(), 1);
    drop(daemon);

    let (distros, second) = distro::local(dir.path().join("distros"), start_here);
    let daemon = Daemon::new(NodeHost::new(hosts).with_distros(distros));
    for id in 1..=4 {
        daemon.host.resume(NodeId(id), |_| {}).unwrap();
    }
    let at = endpoint(dir.path(), "woken");
    daemon
        .host
        .spawn_in(NodeId(5), DISTRO, waiting(dir.path(), &at), |_| {})
        .unwrap();
    shown(&daemon, NodeId(5), "ready");
    assert_eq!(second.launched(), 0, "the running distro host serves them");
    for id in 1..=5 {
        daemon.host.kill(NodeId(id));
    }
    first.end_all();
}

#[test]
fn a_node_outlives_its_distro_host_and_the_daemon_reaches_it_through_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let (distros, local) = distro::local(dir.path().join("distros"), start_here);
    let daemon = Daemon::new(NodeHost::new(dir.path().join("hosts")).with_distros(distros));
    let at = endpoint(dir.path(), "outlives");
    let (exited, exit) = std::sync::mpsc::channel();
    daemon
        .host
        .spawn_in(NODE, DISTRO, waiting(dir.path(), &at), move |e| {
            exited.send(e).unwrap()
        })
        .unwrap();
    shown(&daemon, NODE, "ready");

    local.end_all();
    until(|| local.launched() == 2);
    assert!(ask(&at, "/echo", "after").ends_with("after from 1"));
    shown(&daemon, NODE, "ready");
    assert!(exit.try_recv().is_err(), "the node did not exit");

    daemon.host.kill(NODE);
    let exited = exit.recv_timeout(LIMIT).unwrap();
    assert!(matches!(exited, Exit::Command(_)), "{exited:?}");
    local.end_all();
}

#[test]
fn a_node_whose_pty_host_is_gone_from_its_distro_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = dir.path().join("hosts");
    let (distros, local) = distro::local(dir.path().join("distros"), start_here);
    let closed = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .and_then(|u| u.local_addr())
        .unwrap();
    std::fs::create_dir_all(&hosts).unwrap();
    std::fs::write(hosts.join("1"), format!("{closed}\n{DISTRO}")).unwrap();
    let host = NodeHost::<NodeId>::new(hosts.clone()).with_distros(distros);
    assert_eq!(host.resume(NODE, |_| {}), Err(Unattached::Gone));
    assert!(!hosts.join("1").exists());
    local.end_all();
}
