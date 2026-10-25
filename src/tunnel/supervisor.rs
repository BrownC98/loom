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
            Ok(Err(_)) => Err("the tunnel supervisor ended".into()),
            Err(_) => Err(format!(
                "the tunnel did not start in {} s",
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
                eprintln!("loom: tunnel to {distro} ended: {why}");
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
        return "the tunnel has no pipes".into();
    };
    let name = distro.to_owned();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("loom: tunnel {name}: {line}");
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
        Ok(status) => format!("the relay exited ({status})"),
        Err(e) => e.to_string(),
    }
}
