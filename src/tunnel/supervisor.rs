use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;
use tokio::sync::{Notify, mpsc, watch};

use super::{Event, Stream};

pub trait Launch: Send + Sync + 'static {
    fn command(&self, distro: &str) -> Result<tokio::process::Command, String>;
    fn needed(&self, distro: &str) -> impl Future<Output = bool> + Send;
}

#[derive(Clone, Copy)]
pub struct Pace {
    pub first: Duration,
    pub most: Duration,
    pub stable: Duration,
    pub ready: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace {
            first: Duration::from_secs(1),
            most: Duration::from_secs(60),
            stable: Duration::from_secs(10),
            ready: Duration::from_secs(30),
        }
    }
}

impl Pace {
    fn slower(&self, pause: Duration) -> Duration {
        (pause * 2).clamp(self.first, self.most)
    }

    fn after(&self, pause: Duration, ready: bool, ran: Duration) -> Duration {
        match ready && ran >= self.stable {
            true => Duration::ZERO,
            false => self.slower(pause),
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
    recheck: Notify,
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
        match settled.await {
            Ok(now) => match &now.phase {
                Phase::Down(why) => Err(why.clone()),
                _ => Ok(()),
            },
            Err(_) => Err("터널 감독이 끝났다".into()),
        }
    }

    pub fn recheck(&self, distro: &str) {
        if let Some(supervised) = self.distros.lock().unwrap().get(distro) {
            supervised.recheck.notify_one();
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
            recheck: Notify::new(),
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
    let mut pause = Duration::ZERO;
    let mut kicked = true;
    let mut last = String::new();
    loop {
        if kicked || launch.needed(&distro).await {
            supervised.state.send_modify(|s| {
                s.attempt += 1;
                s.phase = Phase::Starting;
            });
            let began = Instant::now();
            let why = run(&*launch, &doors, &distro, &supervised, pace.ready).await;
            if why != last {
                eprintln!("loom: {distro} 터널이 끝났다: {why}");
            }
            let ready = supervised.state.borrow().phase == Phase::Ready;
            supervised
                .state
                .send_modify(|s| s.phase = Phase::Down(why.clone()));
            last = why;
            pause = pace.after(pause, ready, began.elapsed());
        } else {
            pause = pace.slower(pause);
        }
        kicked = tokio::select! {
            () = tokio::time::sleep(pause) => false,
            () = supervised.kick.notified() => true,
        };
    }
}

async fn run<L: Launch>(
    launch: &L,
    doors: &mpsc::UnboundedSender<Stream>,
    distro: &str,
    supervised: &Supervised,
    ready_wait: Duration,
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
    let deadline = tokio::time::Instant::now() + ready_wait;
    let mut ready = false;
    let end = loop {
        let next = async {
            match ready {
                true => Ok(events.recv().await),
                false => tokio::time::timeout_at(deadline, events.recv()).await,
            }
        };
        let event = tokio::select! {
            got = next => match got {
                Ok(event) => event,
                Err(_) => break End::Late,
            },
            () = supervised.recheck.notified() => match launch.needed(distro).await {
                true => continue,
                false => break End::Unneeded,
            },
        };
        match event {
            None => break End::Exited,
            Some(Event::Ready(version)) if version != super::wire::VERSION => {
                break End::Version(version);
            }
            Some(Event::Ready(_)) => {
                ready = true;
                supervised.state.send_modify(|s| s.phase = Phase::Ready);
            }
            Some(Event::Opened(stream)) => {
                let _ = doors.send(stream);
            }
        }
    };
    drop(events);
    let _ = child.start_kill();
    let ended = child.wait().await;
    match (end, ended) {
        (End::Late, _) => format!("중계가 {ready_wait:?} 안에 준비되지 않았다"),
        (End::Unneeded, _) => format!("{distro}에 도는 노드가 없다"),
        (End::Version(theirs), _) => format!(
            "중계의 프레임 판({theirs})이 데몬의 판({})과 다르다. 훅은 loom.exe로 간다",
            super::wire::VERSION
        ),
        (End::Exited, Ok(status)) => format!("중계가 끝났다 ({status})"),
        (End::Exited, Err(e)) => e.to_string(),
    }
}

enum End {
    Exited,
    Version(u32),
    Late,
    Unneeded,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Fake {
        script: &'static str,
        starts: Arc<AtomicUsize>,
        needed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Launch for Fake {
        fn command(&self, _: &str) -> Result<tokio::process::Command, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let mut cmd = tokio::process::Command::new(SHELL[0]);
            cmd.args([SHELL[1], self.script]);
            Ok(cmd)
        }

        async fn needed(&self, _: &str) -> bool {
            self.needed.load(Ordering::SeqCst)
        }
    }

    #[cfg(unix)]
    const SHELL: [&str; 2] = ["sh", "-c"];
    #[cfg(unix)]
    const READY_THEN_EXIT: &str = r"printf '\000\001\000\000\000\000\000\000\000'; sleep 0.2";
    #[cfg(unix)]
    const BROKEN: &str = "echo broken >&2; exit 7";
    #[cfg(unix)]
    const SILENT: &str = "exec sleep 600";
    #[cfg(windows)]
    const SHELL: [&str; 2] = ["powershell", "-Command"];
    #[cfg(windows)]
    const READY_THEN_EXIT: &str = "$b = [byte[]]::new(9); $b[1] = 1; [Console]::OpenStandardOutput().Write($b, 0, 9); Start-Sleep -Milliseconds 200";
    #[cfg(windows)]
    const BROKEN: &str = "[Console]::Error.WriteLine('broken'); exit 7";
    #[cfg(windows)]
    const SILENT: &str = "Start-Sleep -Seconds 600";

    type Needed = Arc<std::sync::atomic::AtomicBool>;

    const HOUR: Duration = Duration::from_secs(3600);

    const QUICK: Pace = Pace {
        first: Duration::from_millis(10),
        most: Duration::from_millis(20),
        stable: HOUR,
        ready: Duration::from_secs(5),
    };

    fn tunnels(script: &'static str, needed: bool) -> (Tunnels<Fake>, Arc<AtomicUsize>, Needed) {
        paced(script, needed, QUICK)
    }

    fn paced(
        script: &'static str,
        needed: bool,
        pace: Pace,
    ) -> (Tunnels<Fake>, Arc<AtomicUsize>, Needed) {
        let starts = Arc::new(AtomicUsize::new(0));
        let needed = Arc::new(std::sync::atomic::AtomicBool::new(needed));
        let fake = Fake {
            script,
            starts: starts.clone(),
            needed: needed.clone(),
        };
        (
            Tunnels::new(fake, crate::tunnel::Door::new().0, pace),
            starts,
            needed,
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
        let (tunnels, starts, _) = tunnels(READY_THEN_EXIT, true);
        tunnels.ensure("Ubuntu").await.unwrap();
        until(|| starts.load(Ordering::SeqCst) >= 3).await;
        tunnels.ensure("Ubuntu").await.unwrap();
    }

    #[test]
    fn quick_failures_wait_longer_each_time_and_a_long_ready_run_restarts_at_once() {
        let pace = Pace {
            first: Duration::from_secs(1),
            most: Duration::from_secs(8),
            stable: Duration::from_secs(10),
            ready: Duration::from_secs(30),
        };
        let short = Duration::from_secs(2);
        let mut pause = Duration::ZERO;
        let mut pauses = Vec::new();
        for ready in [false, true, false, true, false] {
            pause = pace.after(pause, ready, short);
            pauses.push(pause.as_secs());
        }
        assert_eq!(pauses, [1, 2, 4, 8, 8]);
        assert_eq!(pace.after(pause, true, pace.stable), Duration::ZERO);
        assert_eq!(pace.after(pause, false, pace.ready), pace.most);
        assert_eq!(pace.after(Duration::ZERO, false, short), pace.first);
    }

    #[tokio::test]
    async fn a_relay_that_was_ready_long_enough_starts_again_at_once() {
        let pace = Pace {
            first: HOUR,
            most: HOUR,
            stable: Duration::from_millis(100),
            ready: Duration::from_secs(5),
        };
        let (tunnels, starts, _) = paced(READY_THEN_EXIT, true, pace);
        tunnels.ensure("Ubuntu").await.unwrap();
        until(|| starts.load(Ordering::SeqCst) >= 3).await;
    }

    #[tokio::test]
    async fn a_relay_that_ends_soon_after_it_was_ready_waits_before_it_starts_again() {
        let pace = Pace {
            first: HOUR,
            most: HOUR,
            ..QUICK
        };
        let (tunnels, starts, _) = paced(READY_THEN_EXIT, true, pace);
        tunnels.ensure("Ubuntu").await.unwrap();
        let supervised = tunnels.supervised("Ubuntu");
        until(|| matches!(supervised.state.borrow().phase, Phase::Down(_))).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_tunnel_that_never_gets_ready_is_reported() {
        let (tunnels, _, _) = tunnels(BROKEN, true);
        let why = tunnels.ensure("Ubuntu").await.unwrap_err();
        assert!(why.ends_with(": 7)"), "{why}");
    }

    #[tokio::test]
    async fn a_distro_without_nodes_is_not_started_again_until_a_node_needs_it() {
        let (tunnels, starts, _) = tunnels(READY_THEN_EXIT, false);
        tunnels.ensure("Ubuntu").await.unwrap();
        let supervised = tunnels.supervised("Ubuntu");
        until(|| matches!(supervised.state.borrow().phase, Phase::Down(_))).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        tunnels.ensure("Ubuntu").await.unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_live_relay_that_never_gets_ready_is_ended_and_started_again() {
        let (tunnels, starts, _) = tunnels(SILENT, true);
        let why = tunnels.ensure("Ubuntu").await.unwrap_err();
        assert_eq!(why, "중계가 5s 안에 준비되지 않았다");
        until(|| starts.load(Ordering::SeqCst) >= 2).await;
        let why = tunnels.ensure("Ubuntu").await.unwrap_err();
        assert_eq!(why, "중계가 5s 안에 준비되지 않았다");
    }

    #[cfg(unix)]
    const READY_THEN_STAY: &str = r"printf '\000\001\000\000\000\000\000\000\000'; exec sleep 600";
    #[cfg(unix)]
    const OLD_READY_THEN_STAY: &str =
        r"printf '\000\000\000\000\000\000\000\000\000'; exec sleep 600";
    #[cfg(windows)]
    const READY_THEN_STAY: &str = "$b = [byte[]]::new(9); $b[1] = 1; [Console]::OpenStandardOutput().Write($b, 0, 9); Start-Sleep -Seconds 600";
    #[cfg(windows)]
    const OLD_READY_THEN_STAY: &str =
        "[Console]::OpenStandardOutput().Write([byte[]]::new(9), 0, 9); Start-Sleep -Seconds 600";

    #[tokio::test]
    async fn the_relay_ends_when_the_last_node_of_its_distro_ends() {
        let (tunnels, starts, needed) = tunnels(READY_THEN_STAY, true);
        tunnels.ensure("Ubuntu").await.unwrap();
        tunnels.recheck("Ubuntu");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(tunnels.ensure("Ubuntu").await.is_ok());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        needed.store(false, Ordering::SeqCst);
        tunnels.recheck("Ubuntu");
        let supervised = tunnels.supervised("Ubuntu");
        until(|| matches!(&supervised.state.borrow().phase, Phase::Down(why) if why == "Ubuntu에 도는 노드가 없다")).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        tunnels.ensure("Ubuntu").await.unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_relay_of_another_frame_version_is_ended_and_reported() {
        let (tunnels, starts, _) = tunnels(OLD_READY_THEN_STAY, true);
        let why = tunnels.ensure("Ubuntu").await.unwrap_err();
        assert_eq!(
            why,
            "중계의 프레임 판(0)이 데몬의 판(1)과 다르다. 훅은 loom.exe로 간다"
        );
        until(|| starts.load(Ordering::SeqCst) >= 2).await;
    }
}
