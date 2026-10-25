use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;
use tokio::sync::{Notify, mpsc, watch};

use super::{Event, Stream};

const READY_WAIT: Duration = Duration::from_secs(30);

pub trait Launch: Send + Sync + 'static {
    fn command(&self, distro: &str) -> Result<tokio::process::Command, String>;
    fn running(&self, distro: &str) -> impl Future<Output = bool> + Send;
}

#[derive(Clone, Copy)]
pub struct Pace {
    pub first: Duration,
    pub most: Duration,
    pub stable: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace {
            first: Duration::from_secs(1),
            most: Duration::from_secs(60),
            stable: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Starting,
    Ready,
    Down(String),
}

#[derive(Clone, Debug)]
struct State {
    attempt: u64,
    phase: Phase,
}

struct Supervised {
    state: watch::Sender<State>,
    kick: Notify,
}

pub struct Tunnels<L> {
    launch: Arc<L>,
    doors: mpsc::UnboundedSender<Stream>,
    pace: Pace,
    distros: Mutex<HashMap<String, Arc<Supervised>>>,
}

impl<L: Launch> Tunnels<L> {
    pub fn new(launch: L, doors: mpsc::UnboundedSender<Stream>, pace: Pace) -> Self {
        Tunnels {
            launch: Arc::new(launch),
            doors,
            pace,
            distros: Mutex::default(),
        }
    }

    pub async fn ensure(&self, distro: &str) -> Result<(), String> {
        let supervised = self.supervised(distro);
        let mut state = supervised.state.subscribe();
        let seen = state.borrow_and_update().clone();
        match seen.phase {
            Phase::Ready => return Ok(()),
            Phase::Down(_) => supervised.kick.notify_one(),
            Phase::Starting => {}
        }
        let settled = state.wait_for(|now| match now.phase {
            Phase::Ready => true,
            Phase::Down(_) => now.attempt > seen.attempt || seen.phase == Phase::Starting,
            Phase::Starting => false,
        });
        match tokio::time::timeout(READY_WAIT, settled).await {
            Ok(Ok(now)) => match &now.phase {
                Phase::Down(why) => Err(why.clone()),
                _ => Ok(()),
            },
            Ok(Err(_)) => Err("터널 감독이 끝났다".into()),
            Err(_) => Err(format!(
                "터널이 {}초 안에 뜨지 않았다",
                READY_WAIT.as_secs()
            )),
        }
    }

    fn supervised(&self, distro: &str) -> Arc<Supervised> {
        let mut distros = self.distros.lock().unwrap();
        if let Some(supervised) = distros.get(distro) {
            return supervised.clone();
        }
        let supervised = Arc::new(Supervised {
            state: watch::Sender::new(State {
                attempt: 0,
                phase: Phase::Starting,
            }),
            kick: Notify::new(),
        });
        distros.insert(distro.to_owned(), supervised.clone());
        tokio::spawn(supervise(
            self.launch.clone(),
            self.doors.clone(),
            self.pace,
            distro.to_owned(),
            supervised.clone(),
        ));
        supervised
    }
}

async fn supervise<L: Launch>(
    launch: Arc<L>,
    doors: mpsc::UnboundedSender<Stream>,
    pace: Pace,
    distro: String,
    supervised: Arc<Supervised>,
) {
    let mut pause = pace.first;
    let mut kicked = true;
    let mut last = String::new();
    loop {
        if kicked || launch.running(&distro).await {
            supervised.state.send_modify(|s| {
                s.attempt += 1;
                s.phase = Phase::Starting;
            });
            let began = Instant::now();
            let why = run(&*launch, &doors, &distro, &supervised).await;
            if why != last {
                eprintln!("loom: {distro} 터널이 끝났다: {why}");
            }
            supervised
                .state
                .send_modify(|s| s.phase = Phase::Down(why.clone()));
            last = why;
            if began.elapsed() >= pace.stable {
                pause = pace.first;
            }
        }
        kicked = tokio::select! {
            () = tokio::time::sleep(pause) => false,
            () = supervised.kick.notified() => true,
        };
        pause = (pause * 2).min(pace.most);
    }
}

async fn run<L: Launch>(
    launch: &L,
    doors: &mpsc::UnboundedSender<Stream>,
    distro: &str,
    supervised: &Supervised,
) -> String {
    let mut cmd = match launch.command(distro) {
        Ok(cmd) => cmd,
        Err(why) => return why,
    };
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return e.to_string(),
    };
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return "터널 프로세스에 파이프가 없다".into();
    };
    let name = distro.to_owned();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("loom: {name} 터널: {line}");
        }
    });
    let mut events = super::accept(stdout, stdin);
    while let Some(event) = events.recv().await {
        match event {
            Event::Ready => supervised.state.send_modify(|s| s.phase = Phase::Ready),
            Event::Opened(stream) => {
                let _ = doors.send(stream);
            }
        }
    }
    let _ = child.start_kill();
    match child.wait().await {
        Ok(status) => format!("중계가 끝났다 ({status})"),
        Err(e) => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Fake {
        script: &'static str,
        starts: Arc<AtomicUsize>,
        running: bool,
    }

    impl Launch for Fake {
        fn command(&self, _: &str) -> Result<tokio::process::Command, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let mut cmd = tokio::process::Command::new(SHELL[0]);
            cmd.args([SHELL[1], self.script]);
            Ok(cmd)
        }

        async fn running(&self, _: &str) -> bool {
            self.running
        }
    }

    #[cfg(unix)]
    const SHELL: [&str; 2] = ["sh", "-c"];
    #[cfg(unix)]
    const READY_THEN_EXIT: &str = r"printf '\000\000\000\000\000\000\000\000\000'; sleep 0.2";
    #[cfg(unix)]
    const BROKEN: &str = "echo broken >&2; exit 7";
    #[cfg(windows)]
    const SHELL: [&str; 2] = ["powershell", "-Command"];
    #[cfg(windows)]
    const READY_THEN_EXIT: &str = "[Console]::OpenStandardOutput().Write([byte[]]::new(9), 0, 9); Start-Sleep -Milliseconds 200";
    #[cfg(windows)]
    const BROKEN: &str = "[Console]::Error.WriteLine('broken'); exit 7";

    fn tunnels(script: &'static str, running: bool) -> (Tunnels<Fake>, Arc<AtomicUsize>) {
        let starts = Arc::new(AtomicUsize::new(0));
        let fake = Fake {
            script,
            starts: starts.clone(),
            running,
        };
        let pace = Pace {
            first: Duration::from_millis(10),
            most: Duration::from_millis(20),
            stable: Duration::from_secs(3600),
        };
        (
            Tunnels::new(fake, crate::tunnel::Door::new().0, pace),
            starts,
        )
    }

    async fn until(what: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !what() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_exited_tunnel_starts_again() {
        let (tunnels, starts) = tunnels(READY_THEN_EXIT, true);
        tunnels.ensure("Ubuntu").await.unwrap();
        until(|| starts.load(Ordering::SeqCst) >= 3).await;
        tunnels.ensure("Ubuntu").await.unwrap();
    }

    #[tokio::test]
    async fn a_tunnel_that_never_gets_ready_is_reported() {
        let (tunnels, _) = tunnels(BROKEN, true);
        let why = tunnels.ensure("Ubuntu").await.unwrap_err();
        assert!(why.ends_with(": 7)"), "{why}");
    }

    #[tokio::test]
    async fn a_stopped_distro_is_not_started_again_until_a_node_needs_it() {
        let (tunnels, starts) = tunnels(READY_THEN_EXIT, false);
        tunnels.ensure("Ubuntu").await.unwrap();
        let supervised = tunnels.supervised("Ubuntu");
        until(|| matches!(supervised.state.borrow().phase, Phase::Down(_))).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        tunnels.ensure("Ubuntu").await.unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }
}
