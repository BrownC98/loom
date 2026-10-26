use super::*;

fn no_start(_: Start) -> Result<SocketAddr, String> {
    Err("no pty-host here".into())
}

#[cfg(unix)]
fn sh(script: &str) -> Command {
    Command {
        argv: ["sh", "-c", script].map(str::to_owned).to_vec(),
        cwd: "/".into(),
        env: vec![("WHO".into(), "distro".into())],
    }
}

#[cfg(windows)]
fn sh(script: &str) -> Command {
    let script = script.replace("$WHO", "%WHO%");
    Command {
        argv: ["cmd", "/c", &script].map(str::to_owned).to_vec(),
        cwd: std::env::temp_dir().display().to_string(),
        env: vec![("WHO".into(), "distro".into())],
    }
}

#[test]
fn version_1_bytes_are_frozen() {
    assert_eq!(
        frame(&hello(1)),
        [&[15, 0, 0, 0][..], b"loom-distro", &[1, 0, 0, 0]].concat()
    );
    assert_eq!(version_of(&hello(7)), Some(7));
    assert_eq!(version_of(b"loom-pty\x01\x00\x00\x00"), None);
    let asks = [
        (
            Ask::Start(Start {
                program: "/l".into(),
                command: Command {
                    argv: vec!["sh".into()],
                    cwd: "/w".into(),
                    env: vec![("A".into(), "b".into())],
                },
                setup: Some(vec!["s".into()]),
            }),
            r#"{"start":{"program":"/l","command":{"argv":["sh"],"cwd":"/w","env":[["A","b"]]},"setup":["s"]}}"#,
        ),
        (
            Ask::Reach("127.0.0.1:9".parse().unwrap()),
            r#"{"reach":"127.0.0.1:9"}"#,
        ),
        (
            Ask::Run(Command {
                argv: vec!["true".into()],
                cwd: "/".into(),
                env: Vec::new(),
            }),
            r#"{"run":{"argv":["true"],"cwd":"/","env":[]}}"#,
        ),
        (
            Ask::Serve(Command {
                argv: vec!["codex".into()],
                cwd: "/".into(),
                env: Vec::new(),
            }),
            r#"{"serve":{"argv":["codex"],"cwd":"/","env":[]}}"#,
        ),
    ];
    for (ask, json) in asks {
        assert_eq!(serde_json::to_string(&ask).unwrap(), json);
        assert_eq!(serde_json::from_str::<Ask>(json).unwrap(), ask);
    }
    let answers = [
        (
            Answer::Started("127.0.0.1:9".parse().unwrap()),
            r#"{"started":"127.0.0.1:9"}"#,
        ),
        (Answer::Reached, r#""reached""#),
        (Answer::Gone, r#""gone""#),
        (
            Answer::Ran(Ran {
                code: Some(0),
                stdout: "x".into(),
                stderr: String::new(),
            }),
            r#"{"ran":{"code":0,"stdout":"x","stderr":""}}"#,
        ),
        (Answer::Spawned(7), r#"{"spawned":7}"#),
        (Answer::Exited(Some(3)), r#"{"exited":3}"#),
        (Answer::Failed("no".into()), r#"{"failed":"no"}"#),
    ];
    for (answer, json) in answers {
        assert_eq!(serde_json::to_string(&answer).unwrap(), json);
        assert_eq!(serde_json::from_str::<Answer>(json).unwrap(), answer);
    }
}

#[test]
fn a_distro_host_of_another_version_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let old = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std::fs::write(
        dir.path().join("Ubuntu"),
        old.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let greeted = std::thread::spawn(move || {
        let (mut conn, _) = old.accept().unwrap();
        let asked = read_frame(&mut conn).unwrap();
        write_frame(&mut conn, &hello(DISTRO_HOST + 1)).unwrap();
        version_of(&asked)
    });
    let (distros, local) = local(dir.path().to_path_buf(), no_start);
    let ran = distros.run("Ubuntu", sh("echo $WHO")).unwrap();
    assert_eq!(ran.stdout.trim_end(), "distro");
    assert_eq!(greeted.join().unwrap(), Some(DISTRO_HOST));
    assert_eq!(local.launched(), 1);
    assert_eq!(distros.run("Ubuntu", sh("exit 4")).unwrap().code, Some(4));
    assert_eq!(local.launched(), 1);
    local.end_all();
}

#[cfg(unix)]
#[test]
fn a_served_process_reports_its_pid_and_its_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (distros, local) = local(dir.path().to_path_buf(), no_start);
    let serving = distros.serve("Ubuntu", sh("exec sleep 30")).unwrap();
    assert!(Path::new(&format!("/proc/{}", serving.pid)).exists());
    unsafe { libc::kill(serving.pid as i32, libc::SIGTERM) };
    assert_eq!(serving.exited(), Ok(None));
    let failed = distros.serve(
        "Ubuntu",
        Command {
            argv: vec!["/no/such/program".into()],
            cwd: "/".into(),
            env: Vec::new(),
        },
    );
    assert!(failed.is_err());
    local.end_all();
}

#[cfg(unix)]
#[test]
fn a_distro_host_that_does_not_greet_is_ended_before_the_next_starts() {
    use std::os::unix::process::ExitStatusExt;
    let dir = tempfile::tempdir().unwrap();
    let closed = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .unwrap();
    let spawn = |name: &str| {
        std::process::Command::new("sh")
            .args(["-c", "sleep 30; :", name])
            .spawn()
            .unwrap()
    };
    let (mut silent, mut other) = (spawn("distro-host"), spawn("something-else"));
    let (distros, local) = local(dir.path().to_path_buf(), no_start);
    for (child, launches) in [(&mut silent, 1), (&mut other, 2)] {
        std::fs::write(
            dir.path().join("Ubuntu"),
            format!("{closed} {}", child.id()),
        )
        .unwrap();
        assert_eq!(distros.run("Ubuntu", sh("exit 0")).unwrap().code, Some(0));
        assert_eq!(local.launched(), launches);
    }
    assert_eq!(silent.wait().unwrap().signal(), Some(libc::SIGKILL));
    assert!(
        other.try_wait().unwrap().is_none(),
        "only a distro host is ended"
    );
    other.kill().unwrap();
    other.wait().unwrap();
    local.end_all();
}
