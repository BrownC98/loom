//! 노드 호스트: 키마다 PTY 호스트(`loom pty-host`) 하나에 붙어, 출력을 보는 화면 모두에 나눠 준다.
//! PTY와 프로세스는 호스트 프로세스가 쥔다. 그래서 데몬이 다시 시작해도 노드는 끝나지 않고, 새 데몬이 다시 붙는다.
//! 노드 이름·상태는 모른다. 그것은 graph가 갖는다. 키는 노드 id나 로그인 창 id다.
//! 호스트마다 듣는 주소를 `<폴더>/<키>` 파일에 둔다. 파일은 데몬이 쓰고 지운다.

mod host;
mod wire;

use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::hash::Hash;
use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use portable_pty::CommandBuilder;
use tokio::sync::{broadcast, watch};

use crate::process_tree::{self, Snapshot};
use wire::Frame;

pub use host::run;

/// 다시 연 카드에 먼저 보내는 출력의 상한. 근거: docs/design.md 「규모」.
const BACKLOG_BYTES: usize = 256 * 1024;
/// 화면 하나가 밀려도 버티는 출력 조각 수. 넘으면 그 화면의 연결을 끊고, 화면은 다시 붙어 backlog를 받는다.
const VIEWER_QUEUE_CHUNKS: usize = 1024;
/// 주소 파일의 호스트에 연결하는 상한. 루프백이라 살아 있으면 바로 붙는다.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// 호스트가 끝났는데 종료 코드가 오지 않았을 때 쓰는 코드. 호스트가 죽으면 PTY가 닫혀 노드도 끝난다.
const LOST_CODE: u32 = u32::MAX;

/// 이만큼 출력이 없었으면 backlog 속 색 질의(OSC 10·11·4)는 모두 시한이 지났다. 질의를 기다리는 프로그램은 그 질의가 마지막 출력이다.
/// 근거: OpenCode 2.0.32는 250 ms·1000 ms까지 기다린다(설치본 번들 `QUERY_TIMEOUT_MS`, `waitForThemeMode(1000)`). 그 두 배다.
const SETTLED_AFTER: Duration = Duration::from_secs(2);

/// 친 키 뒤 이만큼 지나야 화면이 그 키를 그렸다고 본다(`Keys::settled_screen`). 출력이 왔다는 것만으로는 모른다: 창 제목만 바꾼 출력도 온다(2026-10-24 검토 codex F2).
/// 근거: 재지 않았다. TUI가 키를 그리는 데는 한 화면 갱신이면 된다. 1초 넘게 친 글이 보이지 않으면 사람도 멈춘 것으로 본다.
pub(crate) const INPUT_SETTLE: Duration = Duration::from_secs(1);

/// 설정 스크립트가 실패했을 때 부모에게 보이는 출력 끝 줄 수. 근거: 오류는 보통 마지막 몇 줄에 있다(npm·tsc·cargo).
const TAIL_LINES: usize = 20;

/// PTY 호스트가 알린 끝.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exit {
    /// 명령이 이 코드로 끝났다. 설정 스크립트가 있었으면 그것이 성공한 뒤의 명령이다.
    Command(u32),
    /// 설정 스크립트가 이 코드로 끝나 명령을 띄우지 않았다.
    Setup(u32),
}

impl Exit {
    fn frame(self) -> Frame {
        match self {
            Exit::Command(code) => Frame::Exit(code),
            Exit::Setup(code) => Frame::SetupFailed(code),
        }
    }
}

pub struct Attached {
    pub backlog: Bytes,
    pub output: broadcast::Receiver<Bytes>,
    /// PTY의 열·줄. 붙을 때의 값을 먼저 보고, 바뀌면 다시 깬다. 모든 화면이 이 크기로 그린다(docs/design.md 「데몬 하나」).
    pub size: watch::Receiver<PtySize>,
    /// 붙을 때 `SETTLED_AFTER` 넘게 출력이 없었다. 화면은 backlog 속 색 질의에 답하지 않는다.
    pub settled: bool,
}

/// PTY 하나의 크기. 화면에 가는 값이다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

pub struct NodeHost<K> {
    /// 호스트 주소 파일의 폴더.
    dir: PathBuf,
    nodes: RwLock<HashMap<K, Arc<Pty>>>,
}

struct Pty {
    /// PTY에서 띄운 프로세스. 중단할 셸이 이 프로세스의 자손인지 볼 때 쓴다.
    /// 설정 스크립트가 끝나 명령이 뜨면 호스트가 새 값을 보낸다.
    pid: Mutex<Option<u32>>,
    /// 중단이 오면 끝낼 셸(`loom guard`가 알린 Bash 도구 셸).
    guarded: Mutex<Vec<u32>>,
    /// 호스트로 가는 연결. 키 입력, 크기, 끝내기를 보낸다.
    host: Mutex<TcpStream>,
    output: Mutex<Output>,
    /// 화면이 마지막으로 알린 PTY 크기(행, 열). `screen`이 출력을 이 크기의 화면에 다시 그린다.
    size: watch::Sender<PtySize>,
    /// 크기를 바꿀 때 늘 이 잠금 안에서 바꾼다. 그래야 돌려줄 때 그 뒤에 다른 화면이 쥐었는지 바르게 안다.
    /// 호스트에 쓰는 동안에는 쥐지 않는다. 읽는 스레드도 이 잠금을 쥔다. 호스트는 backlog를 다 보낸 뒤에야 입력을 읽으므로, 읽는 스레드가 쓰기를 기다리면 서로 막힌다.
    sizing: Mutex<Sizing>,
}

#[derive(Default)]
struct Sizing {
    /// 폰이 잠시 빌린 크기.
    loan: Option<Loan>,
    host: HostSize,
}

/// 호스트가 인사 뒤 알리는 지금 크기를 받았는가. `resume`은 그 프레임을 기다리지 않고 돌아간다.
#[derive(Default, Clone, Copy, PartialEq)]
enum HostSize {
    /// 받지 않았고 화면도 크기를 정하지 않았다. 받으면 그 크기를 쓴다.
    #[default]
    Unreported,
    /// 받기 전에 화면이 크기를 정했다. 받은 크기는 버린다. 호스트는 알린 뒤에 데몬이 보낸 크기를 읽으므로 화면의 크기가 남는다.
    /// 옛 호스트는 알리지 않으므로 계속 이 값이다.
    Claimed,
    /// 받았다. 그 뒤로 데몬의 크기가 호스트의 크기다.
    Reported,
}

/// 폰이 PTY 크기를 빌리기 전의 크기와 지금 빌린 연결.
struct Loan {
    by: Borrower,
    before: PtySize,
}

/// 크기를 빌리는 화면 연결 하나. 연결마다 새로 만든다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Borrower(u64);

impl Borrower {
    pub fn fresh() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// 호스트가 보낸 출력의 사본과 화면 구독. 붙을 때 호스트가 backlog를 먼저 보내므로 다시 시작한 뒤에도 같다.
struct Output {
    backlog: Backlog,
    tx: broadcast::Sender<Bytes>,
    /// 마지막 출력을 받은 때. 다시 시작한 데몬은 호스트의 backlog를 받은 때다.
    last_push: Instant,
    /// 마지막으로 키(`typed`)를 친 때와 치기 직전 화면 글. 터미널 응답은 키가 아니다. 입력 칸을 바꾸지 않는다.
    typed: Option<(Instant, String)>,
}

/// 최근 출력 `BACKLOG_BYTES`. 호스트와 데몬이 같은 규칙으로 쌓는다.
#[derive(Default)]
struct Backlog(VecDeque<u8>);

impl<K: Copy + Eq + Hash + Display + Send + Sync + 'static> NodeHost<K> {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            nodes: RwLock::default(),
        }
    }

    /// 명령을 새 PTY 호스트에서 띄우고 붙는다.
    /// `setup`이 있으면 호스트가 그것을 먼저 돌리고, 성공할 때만 명령을 띄운다.
    /// 프로세스가 끝나면 `on_exit`에 그 끝을 넘긴다.
    pub fn spawn(
        &self,
        id: K,
        cmd: CommandBuilder,
        setup: Option<CommandBuilder>,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), String> {
        let address = start_host(&cmd, setup.as_ref())?;
        std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(self.file(id), &address))
            .map_err(|e| e.to_string())?;
        self.resume(id, on_exit)
    }

    /// 앞 데몬이 띄운 호스트에 다시 붙는다. 호스트가 backlog를 먼저 보낸다. 붙지 못하면 주소 파일을 지운다.
    pub fn resume(&self, id: K, on_exit: impl FnOnce(Exit) + Send + 'static) -> Result<(), String> {
        let file = self.file(id);
        let connected = connect(&file);
        let (mut reader, pid) = match connected {
            Ok(found) => found,
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                return Err(e);
            }
        };
        let host = reader.try_clone().map_err(|e| e.to_string())?;
        let pty = Arc::new(Pty {
            pid: Mutex::new(pid),
            guarded: Mutex::default(),
            host: Mutex::new(host),
            output: Mutex::new(Output {
                backlog: Backlog::default(),
                tx: broadcast::channel(VIEWER_QUEUE_CHUNKS).0,
                last_push: Instant::now(),
                typed: None,
            }),
            size: watch::Sender::new(host::INITIAL_SIZE_VIEW),
            sizing: Mutex::default(),
        });
        self.nodes.write().unwrap().insert(id, pty.clone());
        std::thread::spawn(move || {
            let exit = loop {
                match wire::read(&mut reader) {
                    Ok(Frame::Data(chunk)) => {
                        pty.output.lock().unwrap().push(&chunk);
                    }
                    Ok(Frame::Exit(code)) => break Exit::Command(code),
                    Ok(Frame::SetupFailed(code)) => break Exit::Setup(code),
                    Ok(Frame::Pid(pid)) => *pty.pid.lock().unwrap() = Some(pid),
                    // 호스트가 인사 뒤에 알리는 지금 크기. 데몬이 다시 시작해도 크기가 이어진다. 옛 호스트는 알리지 않아 처음 크기다.
                    Ok(Frame::Resize { cols, rows }) => {
                        let mut sizing = pty.sizing.lock().unwrap();
                        if sizing.host == HostSize::Unreported {
                            pty.size.send_replace(PtySize { cols, rows });
                        }
                        sizing.host = HostSize::Reported;
                    }
                    Ok(_) => {}
                    Err(_) => break Exit::Command(LOST_CODE),
                }
            };
            let _ = std::fs::remove_file(file);
            on_exit(exit);
        });
        Ok(())
    }

    /// 이 데몬이 붙지 않은 호스트를 모두 끝낸다. 데몬이 시작할 때 다시 붙기를 마친 뒤 부른다.
    /// 예: 데몬이 꺼진 동안 지운 노드, 앞 데몬의 로그인 창.
    pub fn end_others(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let ours: Vec<String> = self
            .nodes
            .read()
            .unwrap()
            .keys()
            .map(|id| id.to_string())
            .collect();
        for entry in entries.flatten() {
            if ours.contains(&entry.file_name().to_string_lossy().into_owned()) {
                continue;
            }
            if let Ok((mut host, _)) = connect(&entry.path()) {
                let _ = wire::write(&mut host, &Frame::Kill);
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }

    /// 출력이 없는 것을 확인할 수 있을 때만 자동 정리할 수 있다.
    pub fn has_output(&self, id: K) -> bool {
        self.nodes
            .read()
            .unwrap()
            .get(&id)
            .is_none_or(|pty| !pty.output.lock().unwrap().backlog.0.is_empty())
    }

    /// 최근 출력의 끝 `TAIL_LINES`줄. 터미널 제어 시퀀스를 지우고, 한 줄을 `\r`로 덮어쓴 진행 표시는 마지막 글만 남긴다.
    pub fn tail(&self, id: K) -> String {
        let Some(pty) = self.pty(id) else {
            return String::new();
        };
        let bytes = pty.output.lock().unwrap().backlog.to_vec();
        // `strip`은 `\r`도 지우므로 줄을 먼저 고른다.
        let lines: Vec<String> = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| {
                line.split('\r')
                    .map(strip_ansi_escapes::strip_str)
                    .rfind(|part| !part.trim().is_empty())
            })
            .collect();
        lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
    }

    /// 지금 터미널 화면. backlog를 마지막 PTY 크기의 화면에 다시 그린다. 지운 줄과 덮어쓴 글은 남지 않는다.
    pub fn screen(&self, id: K) -> Screen {
        self.pty(id)
            .map_or_else(Screen::default, |pty| pty.screen())
    }

    /// `Keys::settled_screen`과 같다. 입력 잠금을 쥐지 않는다.
    pub fn settled_screen(&self, id: K) -> Option<Screen> {
        self.pty(id)?.settled_screen()
    }

    /// backlog와 그 뒤의 출력을 빈틈없이 받는다.
    pub fn attach(&self, id: K) -> Option<Attached> {
        let pty = self.pty(id)?;
        let output = pty.output.lock().unwrap();
        Some(Attached {
            backlog: Bytes::from(output.backlog.to_vec()),
            output: output.tx.subscribe(),
            size: pty.size.subscribe(),
            settled: output.settled(Instant::now()),
        })
    }

    /// `pick`이 고른 바이트를 친다. `pick`은 이 PTY의 입력 잠금을 쥔 채 돈다. 그래서 `pick`이 화면을 읽고 고르는 사이에 다른 입력(사람의 키, 다른 선택)이 끼지 않는다.
    /// 화면을 읽은 PTY에 친다. 그 사이 노드를 다시 띄웠으면 새 PTY에 치지 않는다. `pick`이 거절하면 아무것도 치지 않는다.
    /// 근거: 2026-10-23 리뷰 「한 승인에 두 응답」: 화면 읽기·비교와 쓰기 사이에 잠금이 풀려 두 요청이 모두 쳤다.
    pub fn write_if<E>(
        &self,
        id: K,
        pick: impl FnOnce(&Keys<'_>) -> Result<Vec<u8>, E>,
    ) -> Result<(), E> {
        self.write_paced_if(id, Duration::ZERO, |keys| {
            pick(keys).map(|bytes| vec![bytes])
        })
    }

    /// `write_if`와 같다. 다만 `pick`이 고른 조각 사이마다 `gap`을 쉬며 친다. 끝 조각까지 입력 잠금을 쥔다. 그래서 조각 사이에 다른 입력이 끼지 않는다.
    /// 잠금을 쥔 채 잠든다. async 문맥에서는 `spawn_blocking` 안에서 부른다.
    pub fn write_paced_if<E>(
        &self,
        id: K,
        gap: Duration,
        pick: impl FnOnce(&Keys<'_>) -> Result<Vec<Vec<u8>>, E>,
    ) -> Result<(), E> {
        let pty = self.pty(id);
        let mut host = pty.as_ref().map(|pty| pty.host.lock().unwrap());
        let pieces = pick(&Keys(pty.as_deref()))?;
        let (Some(pty), Some(host)) = (pty.as_ref(), host.as_mut()) else {
            return Ok(());
        };
        let before = pieces
            .iter()
            .any(|bytes| typed(bytes))
            .then(|| pty.screen().text);
        for (i, bytes) in pieces.into_iter().enumerate() {
            if i > 0 {
                std::thread::sleep(gap);
            }
            // 호스트가 끝났으면 실패한다. 화면에 따로 알리지 않는다. 상태 이벤트가 exited를 보인다.
            if wire::write(&mut **host, &Frame::Data(bytes)).is_err() {
                break;
            }
        }
        if let Some(before) = before {
            pty.output.lock().unwrap().typed = Some((Instant::now(), before));
        }
        Ok(())
    }

    /// 이 크기를 PTY 크기로 쥔다. 폰이 빌린 크기가 있으면 그 빌림은 끝난다. 돌려줄 때 이 크기를 덮지 않는다.
    pub fn resize(&self, id: K, cols: u16, rows: u16) {
        if let Some(pty) = self.pty(id) {
            pty.change_size(|sizing, _| {
                sizing.loan = None;
                Some(PtySize { cols, rows })
            });
        }
    }

    /// 폰이 보는 동안만 이 크기를 쓴다. `give_back`이 빌리기 전 크기로 돌린다.
    /// 다른 폰이 이미 빌렸으면 처음 빌리기 전 크기를 그대로 두고 빌린 연결만 바꾼다.
    pub fn borrow(&self, id: K, by: Borrower, cols: u16, rows: u16) {
        if let Some(pty) = self.pty(id) {
            pty.change_size(|sizing, now| {
                let before = sizing.loan.as_ref().map_or(now, |l| l.before);
                sizing.loan = Some(Loan { by, before });
                Some(PtySize { cols, rows })
            });
        }
    }

    /// `by`가 아직 빌리고 있으면 빌리기 전 크기로 돌린다. 그 뒤 다른 화면이 크기를 쥐었으면 그대로 둔다.
    pub fn give_back(&self, id: K, by: Borrower) {
        if let Some(pty) = self.pty(id) {
            pty.change_size(|sizing, _| {
                sizing.loan.take_if(|l| l.by == by).map(|loan| loan.before)
            });
        }
    }

    /// 프로세스를 끝내고 PTY를 놓는다. `spawn`의 `on_exit`는 그대로 불린다.
    pub fn kill(&self, id: K) {
        if let Some(pty) = self.nodes.write().unwrap().remove(&id) {
            pty.send(&Frame::Kill);
        }
    }

    /// `caller`를 띄운 셸을 중단 대상으로 둔다. 셸은 `caller`의 조상 가운데 노드 프로세스의 직속 자식이다.
    /// 부모 PID를 쓰지 않는 이유: Windows Git Bash는 명령마다 fork 대리 프로세스를 띄우고, 그 프로세스는 명령과 함께 끝난다.
    /// 노드 프로세스의 자손이 아니면 두지 않는다. 이미 끝난 셸은 이때 목록에서 지운다.
    pub fn guard(&self, id: K, caller: u32) {
        let Some(pty) = self.pty(id) else { return };
        let root = *pty.pid.lock().unwrap();
        let Some(shell) = root.and_then(|root| Snapshot::now().child_on_path(root, caller)) else {
            return;
        };
        let mut shells = pty.guarded.lock().unwrap();
        if !shells.contains(&shell) {
            shells.push(shell);
        }
        process_tree::alive(&mut shells);
    }

    /// 중단 대상 셸과 그 자손 프로세스를 모두 끝내고, 끝낸 프로세스 수를 돌려준다.
    /// 노드 프로세스의 자손인 셸만 끝낸다. 셸이 끝난 뒤 그 PID를 다른 프로세스가 받았을 수 있다.
    pub fn stop_guarded(&self, id: K) -> usize {
        let Some(pty) = self.pty(id) else { return 0 };
        let shells = std::mem::take(&mut *pty.guarded.lock().unwrap());
        let Some(root) = pty.pid.lock().unwrap().filter(|_| !shells.is_empty()) else {
            return 0;
        };
        let snapshot = Snapshot::now();
        snapshot.kill_trees(
            shells
                .into_iter()
                .filter(|&shell| snapshot.descends(shell, root)),
        )
    }

    /// 노드 프로세스의 PID. 호스트가 아직 알리지 않았으면 없다.
    pub fn pid(&self, id: K) -> Option<u32> {
        *self.pty(id)?.pid.lock().unwrap()
    }

    fn pty(&self, id: K) -> Option<Arc<Pty>> {
        self.nodes.read().unwrap().get(&id).cloned()
    }

    fn file(&self, id: K) -> PathBuf {
        self.dir.join(id.to_string())
    }
}

/// `loom pty-host`를 데몬과 따로 사는 프로세스로 띄우고, 듣는 주소를 받는다.
fn start_host(cmd: &CommandBuilder, setup: Option<&CommandBuilder>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // `scripts/host.sh`가 실행 파일을 바꾼 뒤 옛 데몬이 아직 요청을 받으면 Linux는 `<경로> (deleted)`를 준다.
    // 같은 경로의 새 파일로 띄운다. 이 틈에 `loom spawn`이 "pty-host: No such file or directory"로 실패했다(2026-10-22 ktasearch-총괄 보고).
    let exe = exe
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
        .map_or(exe.clone(), PathBuf::from);
    let mut host = std::process::Command::new(exe);
    host.arg("pty-host")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    process_tree::detach(&mut host);
    let mut child = host.spawn().map_err(|e| format!("pty-host: {e}"))?;
    let spec = serde_json::to_vec(&host::Spec::of(cmd, setup)).map_err(|e| e.to_string())?;
    let sent = child
        .stdin
        .take()
        .expect("stdin을 piped로 열었다")
        .write_all(&spec);
    let mut line = String::new();
    let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
    let read = std::io::BufReader::new(stdout).read_line(&mut line);
    // 이 데몬이 띄운 호스트는 끝날 때 이 데몬이 거둔다. 다시 시작한 데몬의 호스트는 OS가 거둔다.
    std::thread::spawn(move || child.wait());
    sent.and(read).map_err(|e| format!("pty-host: {e}"))?;
    match line.trim() {
        "" => Err("pty-host: exited before announcing its address".into()),
        address => Ok(address.to_owned()),
    }
}

/// 주소 파일의 호스트에 붙고 인사한다. 호스트의 PID를 돌려준다.
fn connect(file: &std::path::Path) -> Result<(TcpStream, Option<u32>), String> {
    let address = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let address = address
        .trim()
        .parse()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let mut stream =
        TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let pid = wire::greet(&mut stream).map_err(|e| e.to_string())?;
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    Ok((stream, pid))
}

/// `write_if`의 `pick`이 보는 PTY. PTY가 없으면 화면이 비었다.
pub struct Keys<'a>(Option<&'a Pty>);

impl Keys<'_> {
    pub fn screen(&self) -> Screen {
        self.0.map_or_else(Screen::default, Pty::screen)
    }

    /// 마지막으로 친 키를 그린 지금 화면. 키 뒤 `INPUT_SETTLE`이 지났고 화면 글이 키를 치기 전과 다르다. 아니면 화면이 아직 그 키를 보이지 않을 수 있다.
    /// 그 키를 TUI가 무시해 화면이 그대로면, 다른 출력이 화면을 바꿀 때까지 없다.
    pub fn settled_screen(&self) -> Option<Screen> {
        self.0.and_then(Pty::settled_screen)
    }
}

impl Pty {
    fn settled_screen(&self) -> Option<Screen> {
        let typed = self.output.lock().unwrap().typed.clone();
        let screen = self.screen();
        typed
            .is_none_or(|(at, before)| at.elapsed() >= INPUT_SETTLE && screen.text != before)
            .then_some(screen)
    }

    fn screen(&self) -> Screen {
        let PtySize { cols, rows } = *self.size.borrow();
        let bytes = self.output.lock().unwrap().backlog.to_vec();
        let size = |n: u16| NonZeroU16::new(n.max(1)).expect("1 이상이다");
        let mut parser = vt100::Parser::new(size(rows), size(cols), 0);
        parser.process(&bytes);
        Screen {
            text: parser.screen().contents(),
            cols,
        }
    }

    fn send(&self, frame: &Frame) {
        let _ = wire::write(&mut *self.host.lock().unwrap(), frame);
    }

    /// `change`가 지금 크기를 보고 고른 크기로 바꾼다. 바뀌었으면 화면과 호스트에 알린다.
    /// 호스트 잠금을 먼저 쥔다. 그래야 호스트가 받는 순서가 바꾼 순서와 같다.
    fn change_size(&self, change: impl FnOnce(&mut Sizing, PtySize) -> Option<PtySize>) {
        let mut host = self.host.lock().unwrap();
        let next = {
            let mut sizing = self.sizing.lock().unwrap();
            let Some(next) = change(&mut sizing, *self.size.borrow()) else {
                return;
            };
            let changed = self
                .size
                .send_if_modified(|size| std::mem::replace(size, next) != next);
            // 처음 쥘 때는 데몬의 크기가 호스트의 크기와 다를 수 있다. 그래서 같아도 보낸다.
            let first = sizing.host == HostSize::Unreported;
            if first {
                sizing.host = HostSize::Claimed;
            }
            (changed || first).then_some(next)
        };
        if let Some(PtySize { cols, rows }) = next {
            let _ = wire::write(&mut *host, &Frame::Resize { cols, rows });
        }
    }
}

impl Output {
    fn settled(&self, now: Instant) -> bool {
        now.duration_since(self.last_push) >= SETTLED_AFTER
    }

    fn push(&mut self, chunk: &[u8]) {
        self.backlog.push(chunk);
        self.last_push = Instant::now();
        // 보는 화면이 없으면 send가 실패한다. backlog에 남았으므로 버려도 된다.
        let _ = self.tx.send(Bytes::copy_from_slice(chunk));
    }
}

impl Backlog {
    fn push(&mut self, chunk: &[u8]) {
        let overflow = (self.0.len() + chunk.len()).saturating_sub(BACKLOG_BYTES);
        self.0.drain(..overflow.min(self.0.len()));
        let keep = chunk.len().min(BACKLOG_BYTES);
        self.0.extend(&chunk[chunk.len() - keep..]);
    }

    fn to_vec(&self) -> Vec<u8> {
        self.0.iter().copied().collect()
    }
}

/// 터미널 화면의 글과 그 폭(칸 수). CLI가 폭에 맞춰 넘긴 줄을 이으려면 폭이 필요하다(`screen::prompt`).
#[derive(Default)]
pub struct Screen {
    pub text: String,
    pub cols: u16,
}

/// 화면이 보낸 입력에 사람이 누른 키가 있다. 터미널이 스스로 보내는 응답은 키가 아니다:
/// 초점(`CSI I`·`CSI O`), 커서 위치(`CSI … R`), 장치 속성(`CSI ? … c`, `CSI > … c`), 상태(`CSI … n`), 모드(`CSI … $ y`),
/// 키보드 플래그(`CSI ? … u`), 창 크기(`CSI … t`), 마우스(`CSI < … M/m`, `CSI M xyz`), 문자열 응답(OSC·DCS·APC).
pub fn typed(input: &[u8]) -> bool {
    let mut rest = input;
    while !rest.is_empty() {
        let Some(len) = report_len(rest) else {
            return true;
        };
        rest = &rest[len..];
    }
    false
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// `seq`가 터미널 응답으로 시작하면 그 길이.
fn report_len(seq: &[u8]) -> Option<usize> {
    if seq.first() != Some(&ESC) {
        return None;
    }
    match seq.get(1)? {
        // OSC·DCS·APC: BEL이나 ST(`ESC \`)까지.
        b']' | b'P' | b'_' => {
            let end = seq[2..]
                .iter()
                .enumerate()
                .find(|&(i, &b)| b == BEL || (b == ESC && seq.get(i + 3) == Some(&b'\\')))?;
            Some(2 + end.0 + if *end.1 == BEL { 1 } else { 2 })
        }
        b'[' => {
            let body = &seq[2..];
            // X10 마우스: `CSI M`과 세 바이트.
            if body.first() == Some(&b'M') {
                return (body.len() >= 4).then_some(6);
            }
            let end = body.iter().position(|b| (0x40..=0x7e).contains(b))?;
            let (params, last) = (&body[..end], body[end]);
            let prefixed = |p: u8| params.first() == Some(&p);
            let report = match last {
                b'I' | b'O' => params.is_empty(),
                b'R' | b'n' | b't' => true,
                b'c' => prefixed(b'?') || prefixed(b'>'),
                b'y' => params.ends_with(b"$"),
                b'u' => prefixed(b'?'),
                b'M' | b'm' => prefixed(b'<'),
                _ => false,
            };
            report.then_some(2 + end + 1)
        }
        _ => None,
    }
}

/// 호스트를 이 테스트 프로세스 안에 띄우고 주소 파일을 쓴다. `loom pty-host`와 같은 코드다.
#[cfg(all(test, unix))]
pub fn in_process(dir: &std::path::Path, id: impl Display, cmd: CommandBuilder) {
    let started = host::start(cmd, None).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(id.to_string()), started.address.to_string()).unwrap();
    std::thread::spawn(move || started.wait());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text`가 나올 때까지 출력을 읽는다.
    #[cfg(unix)]
    fn read_until(attached: &mut Attached, text: &str) -> String {
        let mut seen = String::from_utf8_lossy(&attached.backlog).into_owned();
        while !seen.contains(text) {
            let chunk = attached.output.blocking_recv().unwrap();
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
        seen
    }

    #[cfg(unix)]
    #[test]
    fn terminal_output_is_available_before_exit_callback() {
        for (script, expected) in [("exit 0", false), ("printf 'review this'; exit 0", true)] {
            let dir = tempfile::tempdir().unwrap();
            let mut cmd = CommandBuilder::new("sh");
            cmd.args(["-c", script]);
            in_process(dir.path(), 1, cmd);
            let nodes = Arc::new(NodeHost::new(dir.path().to_path_buf()));
            let observed = nodes.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            nodes
                .resume(1, move |code| {
                    tx.send((code, observed.has_output(1))).unwrap();
                })
                .unwrap();
            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
                (Exit::Command(0), expected)
            );
            assert!(nodes.has_output(99));
            nodes.kill(1);
        }
    }

    /// 폰이 빌린 크기는 돌려주면 빌리기 전 크기로 돌아간다. 그 사이 다른 화면이 쥐면 그 화면의 크기가 남는다.
    #[cfg(unix)]
    #[test]
    fn a_borrowed_size_goes_back_unless_another_screen_claimed() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "read line"]);
        in_process(dir.path(), 1, cmd);
        let nodes = NodeHost::new(dir.path().to_path_buf());
        nodes.resume(1, |_| {}).unwrap();
        let size = nodes.attach(1).unwrap().size;
        let now = || *size.borrow();
        let pc = PtySize {
            cols: 120,
            rows: 40,
        };
        let phone = PtySize { cols: 50, rows: 30 };

        nodes.resize(1, pc.cols, pc.rows);
        let first = Borrower::fresh();
        nodes.borrow(1, first, phone.cols, phone.rows);
        assert_eq!(now(), phone);
        nodes.borrow(1, first, 52, 31);
        nodes.give_back(1, first);
        assert_eq!(now(), pc, "phone closed: the PC size comes back");

        let second = Borrower::fresh();
        nodes.borrow(1, second, phone.cols, phone.rows);
        nodes.resize(1, 100, 30);
        nodes.give_back(1, second);
        assert_eq!(
            now(),
            PtySize {
                cols: 100,
                rows: 30
            },
            "the PC claimed after the phone"
        );

        let (a, b) = (Borrower::fresh(), Borrower::fresh());
        nodes.borrow(1, a, phone.cols, phone.rows);
        nodes.borrow(1, b, 60, 20);
        nodes.give_back(1, a);
        assert_eq!(
            now(),
            PtySize { cols: 60, rows: 20 },
            "a phone that lost the size gives nothing back"
        );
        nodes.give_back(1, b);
        assert_eq!(
            now(),
            PtySize {
                cols: 100,
                rows: 30
            }
        );
        nodes.kill(1);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_daemon_reattaches_with_earlier_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "echo before; read line; echo got $line; exit 3"]);
        in_process(dir.path(), 1, cmd);
        let first = NodeHost::new(dir.path().to_path_buf());
        first.resume(1, |_| {}).unwrap();
        read_until(&mut first.attach(1).unwrap(), "before");
        let first_pid = *first.pty(1).unwrap().pid.lock().unwrap();

        // 다시 시작한 데몬. 앞 데몬의 연결은 호스트가 닫는다.
        let second = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        second
            .resume(1, move |code| tx.send(code).unwrap())
            .unwrap();
        assert_eq!(*second.pty(1).unwrap().pid.lock().unwrap(), first_pid);
        // backlog는 붙은 직후에 오므로 `attach`의 backlog나 그 뒤 출력 가운데 하나로 온다.
        let mut attached = second.attach(1).unwrap();
        second
            .write_if(1, |_| Ok::<_, std::convert::Infallible>(b"x\n".to_vec()))
            .unwrap();
        assert!(read_until(&mut attached, "got x").contains("before"));
        assert_eq!(rx.recv().unwrap(), Exit::Command(3));
        assert!(!dir.path().join("1").exists());
    }

    /// 설정 스크립트가 성공하면 새 PTY에서 명령이 뜨고(출력은 한 터미널로 이어진다), 데몬은 명령의 PID를 받는다.
    /// 실패하면 명령을 띄우지 않고 `Exit::Setup`으로 끝난다. `tail`은 설정 출력의 끝을 제어 시퀀스 없이 돌려준다.
    #[cfg(unix)]
    #[test]
    fn setup_runs_first_and_gates_the_command() {
        let sh = |script: &str| {
            let mut cmd = CommandBuilder::new("sh");
            cmd.args(["-c", script]);
            cmd
        };
        for (setup, exit, has_agent) in [
            ("echo setup-ok", Exit::Command(4), true),
            (
                "printf 'progress 1\\rprogress 2\\n\\033[31msetup-bad\\033[0m\\n'; exit 3",
                Exit::Setup(3),
                false,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let started = host::start(sh("echo agent $$; exit 4"), Some(sh(setup))).unwrap();
            std::fs::write(dir.path().join("1"), started.address.to_string()).unwrap();
            std::thread::spawn(move || started.wait());
            let nodes = NodeHost::new(dir.path().to_path_buf());
            let (tx, rx) = std::sync::mpsc::channel();
            nodes.resume(1, move |code| tx.send(code).unwrap()).unwrap();
            let mut attached = nodes.attach(1).unwrap();
            let code = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let mut seen = String::from_utf8_lossy(&attached.backlog).into_owned();
            while let Ok(chunk) = attached.output.try_recv() {
                seen.push_str(&String::from_utf8_lossy(&chunk));
            }
            assert_eq!(code, exit, "{seen}");
            assert!(seen.contains("setup-"), "{seen}");
            assert_eq!(seen.contains("agent "), has_agent, "{seen}");
            if !has_agent {
                assert_eq!(nodes.tail(1), "progress 2\nsetup-bad");
            }
            if has_agent {
                let agent: u32 = seen
                    .split("agent ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(*nodes.pty(1).unwrap().pid.lock().unwrap(), Some(agent));
            }
        }
    }

    /// 화면이 크기를 쥐고 큰 입력을 쓰는 동안 호스트가 늦은 크기와 큰 backlog를 보내도 막히지 않는다.
    /// 호스트는 backlog를 다 보낸 뒤에야 입력을 읽는다. 읽는 스레드가 쓰는 쪽 잠금을 기다리면 셋이 서로 기다린다.
    #[test]
    fn a_late_host_size_during_a_blocked_write_does_not_deadlock() {
        // 루프백 소켓 버퍼보다 크다. 그래야 양쪽 쓰기가 상대가 읽기 전에 막힌다.
        const FRAMES: usize = 16;
        const FRAME: usize = 1 << 20;
        let limit = Duration::from_secs(20);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1"),
            listener.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let (writing_tx, writing) = std::sync::mpsc::channel::<()>();
        let (host_tx, host_done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            wire::welcome(&mut stream, None).unwrap();
            writing.recv().unwrap();
            wire::write(&mut stream, &Frame::Resize { cols: 80, rows: 24 }).unwrap();
            for _ in 0..FRAMES {
                wire::write(&mut stream, &Frame::Data(vec![b'o'; FRAME])).unwrap();
            }
            let (mut sizes, mut input) = (Vec::new(), 0);
            while input < FRAMES * FRAME {
                match wire::read(&mut stream).unwrap() {
                    Frame::Data(bytes) => input += bytes.len(),
                    Frame::Resize { cols, rows } => sizes.push(PtySize { cols, rows }),
                    _ => {}
                }
            }
            host_tx.send(sizes).unwrap();
        });
        let nodes = Arc::new(NodeHost::new(dir.path().to_path_buf()));
        nodes.resume(1, |_| {}).unwrap();
        let claimed = PtySize {
            cols: 100,
            rows: 40,
        };
        nodes.resize(1, claimed.cols, claimed.rows);
        let (writer_tx, writer_done) = std::sync::mpsc::channel();
        let writer = nodes.clone();
        std::thread::spawn(move || {
            for _ in 0..FRAMES {
                writer
                    .write_if(1, |_| Ok::<_, std::convert::Infallible>(vec![b'i'; FRAME]))
                    .unwrap();
            }
            writer_tx.send(()).unwrap();
        });
        // 쓰는 스레드가 호스트 잠금을 쥐고 막힐 시간을 준다. 늦으면 옛 코드에서도 통과할 뿐, 고친 코드의 결과는 같다.
        std::thread::sleep(Duration::from_millis(300));
        writing_tx.send(()).unwrap();
        writer_done
            .recv_timeout(limit)
            .expect("the daemon write finished");
        let sizes = host_done
            .recv_timeout(limit)
            .expect("the host read all input");
        assert_eq!(
            sizes.last(),
            Some(&claimed),
            "the host keeps the claimed size"
        );
        assert_eq!(*nodes.attach(1).unwrap().size.borrow(), claimed);
    }

    /// 판 1 호스트(이 판 전에 띄운 노드)는 `Pid`를 보내지 않는다. 판 2 데몬은 인사의 PID로 그대로 다룬다.
    #[test]
    fn a_version_1_host_still_works() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1"),
            listener.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(matches!(
                wire::read(&mut stream).unwrap(),
                Frame::Hello { .. }
            ));
            let hello = Frame::Hello {
                version: 1,
                pid: Some(42),
            };
            wire::write(&mut stream, &hello).unwrap();
            wire::write(&mut stream, &Frame::Data(b"old host".to_vec())).unwrap();
            assert_eq!(wire::read(&mut stream).unwrap(), Frame::Data(b"x".to_vec()));
            wire::write(&mut stream, &Frame::Exit(5)).unwrap();
        });
        let nodes = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        nodes.resume(1, move |code| tx.send(code).unwrap()).unwrap();
        assert_eq!(*nodes.pty(1).unwrap().pid.lock().unwrap(), Some(42));
        nodes
            .write_if(1, |_| Ok::<_, std::convert::Infallible>(b"x".to_vec()))
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Exit::Command(5)
        );
        let attached = nodes.attach(1).unwrap();
        assert_eq!(&attached.backlog[..], b"old host");
        host.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stop_guarded_ends_the_shell_and_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        // 노드(sh) 안의 셸(sh)이 자식 둘을 띄운다. 셸이 자기 PID를 출력한다.
        cmd.args(["-c", "sh -c 'echo $$; sleep 30 & sleep 30' ; sleep 30"]);
        in_process(dir.path(), 1, cmd);
        let host = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        host.resume(1, move |code| tx.send(code).unwrap()).unwrap();
        let text = read_until(&mut host.attach(1).unwrap(), "\n");
        let shell: u32 = text.trim().parse().unwrap();
        // 다른 노드의 셸로 알리면 끝내지 않는다.
        host.guard(1, std::process::id());
        host.guard(1, shell);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(host.stop_guarded(1), 3);
        assert_eq!(host.stop_guarded(1), 0);
        host.kill(1);
        rx.recv().unwrap();
    }

    /// 호스트는 데몬이 본 환경 전체로 띄운다. 호스트 자신의 환경은 섞지 않는다.
    #[cfg(unix)]
    #[test]
    fn spec_carries_argv_cwd_and_the_whole_environment() {
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "echo $LOOM_TOKEN"]);
        cmd.cwd("/tmp");
        cmd.env("LOOM_TOKEN", "t1");
        cmd.env_remove("HOME");
        let json = serde_json::to_string(&host::Spec::of(&cmd, None)).unwrap();
        let back: host::Spec = serde_json::from_str(&json).unwrap();
        let (back, setup) = back.commands();
        assert!(setup.is_none());
        assert_eq!(back.get_argv(), cmd.get_argv());
        assert_eq!(back.get_cwd().unwrap(), "/tmp");
        assert_eq!(back.get_env("LOOM_TOKEN").unwrap(), "t1");
        assert_eq!(back.get_env("HOME"), None);
        assert_eq!(
            back.iter_full_env_as_str().count(),
            cmd.iter_full_env_as_str().count()
        );
    }

    #[test]
    fn output_settles_after_quiet_time() {
        let mut output = Output {
            backlog: Backlog::default(),
            tx: broadcast::channel(1).0,
            last_push: Instant::now(),
            typed: None,
        };
        output.push(b"\x1b]11;?\x1b\\");
        let pushed = output.last_push;
        assert!(
            !output.settled(pushed),
            "방금 보낸 질의는 아직 기다릴 수 있다"
        );
        assert!(!output.settled(pushed + SETTLED_AFTER - Duration::from_millis(1)));
        assert!(
            output.settled(pushed + SETTLED_AFTER),
            "조용해진 뒤의 질의는 시한이 지났다"
        );
    }

    #[test]
    fn backlog_keeps_only_the_newest_bytes() {
        let mut backlog = Backlog::default();
        backlog.push(&vec![b'a'; BACKLOG_BYTES]);
        backlog.push(b"xyz");
        let bytes = backlog.to_vec();
        assert_eq!(bytes.len(), BACKLOG_BYTES);
        assert_eq!(&bytes[BACKLOG_BYTES - 4..], b"axyz");
    }

    #[test]
    fn keys_are_typed_but_terminal_replies_are_not() {
        for key in [
            &b"y"[..],
            b"\r",
            b"\x1b",
            b"\x1b[A",
            b"\x1bOB",
            b"\x1b[97u",
            b"\x1b[200~hi\x1b[201~",
            b"\x1b[I1",
        ] {
            assert!(typed(key), "{key:?}");
        }
        for reply in [
            &b"\x1b[I"[..],
            b"\x1b[O",
            b"\x1b[12;40R",
            b"\x1b[?1;2c",
            b"\x1b[>0;276;0c",
            b"\x1b[0n",
            b"\x1b[?2004;1$y",
            b"\x1b[?7u",
            b"\x1b[<0;10;5M\x1b[<0;10;5m",
            b"\x1b]11;rgb:0000/0000/0000\x1b\\",
            b"\x1b]10;rgb:ffff/ffff/ffff\x07",
        ] {
            assert!(!typed(reply), "{reply:?}");
        }
    }
}
