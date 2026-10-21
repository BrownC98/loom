//! PTY 호스트(`loom pty-host`): PTY 하나와 그 프로세스, 최근 출력을 쥐고 데몬보다 오래 산다.
//! 데몬이 다시 시작하거나 새 실행 파일로 바뀌어도 노드 프로세스는 끝나지 않는다. 근거: docs/design.md 「에이전트 실행」.
//! 띄울 명령은 stdin의 JSON(`CommandBuilder`)으로 받고, 듣는 주소를 stdout 한 줄로 알린다. 그 뒤로 stdout은 쓰지 않는다.
//! 설정 스크립트가 있으면 같은 PTY에서 먼저 돌리고, 성공하면 명령을 띄운다. 근거: docs/design.md 「저장소와 워크트리」.
//! 데몬 연결은 한 번에 하나다. 새 연결이 오면 앞 연결을 닫는다(다시 시작한 데몬이 앞 데몬의 자리를 받는다).

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{
    Child, ChildKiller, CommandBuilder, MasterPty, PtySize, SlavePty, native_pty_system,
};
use serde::{Deserialize, Serialize};

use super::wire::{self, Frame};
use super::{Backlog, Exit};

/// 화면이 크기를 보내기 전에 쓰는 PTY 크기.
const INITIAL_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};
/// `NodeHost::screen`이 화면이 크기를 알리기 전에 쓰는 크기(행, 열). 다시 시작한 데몬도 이 값에서 시작한다.
pub(super) const INITIAL_ROWS_COLS: (u16, u16) = (INITIAL_SIZE.rows, INITIAL_SIZE.cols);
/// Windows ConPTY는 첫 출력으로 커서 위치를 묻고, 답이 올 때까지 프로그램을 멈춘다.
/// portable-pty가 `PSEUDOCONSOLE_INHERIT_CURSOR`를 늘 켜고 연다. 화면이 붙지 않은 노드(`loom spawn` 자식)도 뜨도록
/// 호스트가 왼쪽 위로 답하고 이 질의를 출력에서 지운다. 그래서 나중에 붙은 xterm이 한 번 더 답하지 않는다.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
const CURSOR_AT_ORIGIN: &[u8] = b"\x1b[1;1R";
/// 프로세스가 끝난 뒤 남은 출력을 읽는 상한. 자손이 PTY를 쥐고 있으면 읽기가 끝나지 않는다.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(1);
/// 프로세스가 끝났을 때 붙은 데몬이 없으면 종료 코드를 들고 기다리는 시간.
/// 근거: 데몬을 다시 빌드하고 다시 띄우는 동안 끝난 노드도 코드를 남긴다. 넘으면 데몬은 코드 없는 exited로 본다.
const EXIT_KEEP: Duration = Duration::from_secs(600);
/// 연결한 쪽이 첫 프레임을 보내는 상한. 다른 프로그램이 연결해 아무것도 보내지 않아도 스레드가 남지 않는다.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

struct Host {
    process: Mutex<Process>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Box<dyn Write + Send>>,
    state: Mutex<State>,
    /// 종료 코드를 데몬에 넘기면 한 번 보낸다.
    delivered: mpsc::Sender<()>,
}

/// 지금 PTY에서 도는 프로세스(설정 스크립트나 명령)를 끝내는 손잡이.
struct Process {
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// 데몬이 끝내기를 보냈다. 설정 스크립트 뒤의 명령을 띄우지 않는다.
    killed: bool,
}

#[derive(Default)]
struct State {
    /// 지금 PTY에서 도는 프로세스의 PID.
    pid: Option<u32>,
    backlog: Backlog,
    /// 지금 붙은 데몬. 번호는 끊긴 연결이 새 연결을 지우지 않게 가린다.
    client: Option<(u64, TcpStream)>,
    next_client: u64,
    exit: Option<Exit>,
}

/// 데몬이 stdin으로 넘기는 명령. `CommandBuilder`는 env 맵의 키가 `OsString`이라 JSON으로 바꿀 수 없어 값만 옮긴다.
/// 환경은 데몬이 본 전체 환경이다. 호스트의 환경은 쓰지 않는다.
/// `setup`은 명령 앞에 같은 폴더·환경으로 돌릴 설정 스크립트의 `argv`다. 없는 필드는 옛 데몬과 같은 뜻(없음)이다.
#[derive(Serialize, Deserialize)]
pub struct Spec {
    argv: Vec<OsString>,
    cwd: Option<OsString>,
    env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    setup: Option<Vec<OsString>>,
}

impl Spec {
    pub fn of(cmd: &CommandBuilder, setup: Option<&CommandBuilder>) -> Self {
        Self {
            argv: cmd.get_argv().clone(),
            cwd: cmd.get_cwd().cloned(),
            env: cmd
                .iter_full_env_as_str()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            setup: setup.map(|s| s.get_argv().clone()),
        }
    }

    /// 명령과 설정 스크립트. 빈 `argv`는 기본 셸이다(`CommandBuilder::new_default_prog`와 같다).
    pub(super) fn commands(self) -> (CommandBuilder, Option<CommandBuilder>) {
        let build = |argv| {
            let mut cmd = CommandBuilder::from_argv(argv);
            cmd.env_clear();
            for (key, value) in &self.env {
                cmd.env(key, value);
            }
            if let Some(cwd) = &self.cwd {
                cmd.cwd(cwd);
            }
            cmd
        };
        (build(self.argv), self.setup.map(build))
    }
}

pub fn run() -> Result<(), String> {
    let spec: Spec =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("command: {e}"))?;
    let (cmd, setup) = spec.commands();
    let started = start(cmd, setup)?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", started.address)
        .and_then(|()| stdout.flush())
        .map_err(|e| e.to_string())?;
    started.wait();
    Ok(())
}

/// 프로세스를 띄우고 데몬 연결을 받기 시작한 호스트.
pub struct Started {
    pub address: SocketAddr,
    host: Arc<Host>,
    child: Box<dyn Child + Send + Sync>,
    /// 설정 스크립트가 도는 동안: 그 뒤에 띄울 명령과 PTY.
    next: Option<(CommandBuilder, Box<dyn SlavePty + Send>)>,
    /// 출력 읽기가 끝나면 닫힌다.
    output_done: mpsc::Receiver<()>,
    delivered: mpsc::Receiver<()>,
}

/// `setup`이 있으면 그것을 먼저 띄우고, `cmd`는 `Started::wait`가 설정이 성공한 뒤 띄운다.
pub fn start(cmd: CommandBuilder, setup: Option<CommandBuilder>) -> Result<Started, String> {
    let pair = native_pty_system()
        .openpty(INITIAL_SIZE)
        .map_err(|e| e.to_string())?;
    let (first, next) = match setup {
        Some(setup) => (setup, Some(cmd)),
        None => (cmd, None),
    };
    let child = pair.slave.spawn_command(first).map_err(|e| e.to_string())?;
    // 설정 뒤에 띄울 명령이 없으면 PTY를 놓는다. 그래야 프로세스가 끝날 때 출력 읽기가 끝난다.
    let next = match next {
        Some(cmd) => Some((cmd, pair.slave)),
        None => {
            drop(pair.slave);
            None
        }
    };
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let input = pair.master.take_writer().map_err(|e| e.to_string())?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    let (delivered_tx, delivered) = mpsc::channel();
    let host = Arc::new(Host {
        process: Mutex::new(Process {
            killer: child.clone_killer(),
            killed: false,
        }),
        master: Mutex::new(pair.master),
        input: Mutex::new(input),
        state: Mutex::new(State {
            pid: child.process_id(),
            ..State::default()
        }),
        delivered: delivered_tx,
    });
    let (drained, output_done) = mpsc::channel::<()>();
    let pty = host.clone();
    std::thread::spawn(move || {
        pty.read_output(reader);
        drop(drained);
    });
    let accepting = host.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let host = accepting.clone();
            std::thread::spawn(move || host.serve(stream));
        }
    });
    Ok(Started {
        address,
        host,
        child,
        next,
        output_done,
        delivered,
    })
}

impl Started {
    /// 프로세스가 끝날 때까지 기다리고, 끝을 데몬에 넘긴 뒤(또는 `EXIT_KEEP` 뒤) 돌아온다.
    /// 설정 스크립트가 성공하면 명령을 띄우고 그 명령을 기다린다. 실패하면 명령을 띄우지 않고 `Exit::Setup`으로 끝난다.
    pub fn wait(mut self) {
        let code = exit_code(&mut self.child);
        let mut note = None;
        let exit = match self.next.take() {
            None => Exit::Command(code),
            Some(_) if code != 0 => Exit::Setup(code),
            Some((cmd, slave)) => match self.host.replace(&*slave, cmd) {
                Ok(Some(child)) => {
                    drop(slave);
                    self.child = child;
                    Exit::Command(exit_code(&mut self.child))
                }
                Ok(None) => Exit::Command(code),
                Err(e) => {
                    note = Some(format!("loom: 에이전트를 띄우지 못했다: {e}"));
                    Exit::Command(u32::MAX)
                }
            },
        };
        let _ = self.output_done.recv_timeout(DRAIN_AFTER_EXIT);
        if let Some(note) = note {
            self.host.note(&note);
        }
        self.host.exited(exit);
        let _ = self.delivered.recv_timeout(EXIT_KEEP);
    }
}

fn exit_code(child: &mut Box<dyn Child + Send + Sync>) -> u32 {
    child.wait().map(|s| s.exit_code()).unwrap_or(u32::MAX)
}

impl Host {
    fn read_output(&self, mut reader: Box<dyn Read + Send>) {
        let mut buf = [0u8; 8192];
        let mut first = true;
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            let mut chunk = &buf[..n];
            if std::mem::take(&mut first) && cfg!(windows) {
                chunk = self.answer_cursor_query(chunk);
            }
            let mut state = self.state.lock().unwrap();
            state.backlog.push(chunk);
            state.send(&Frame::Data(chunk.to_vec()));
        }
    }

    /// `chunk`가 ConPTY의 커서 위치 질의로 시작하면 답하고 질의 뒤의 출력을 돌려준다.
    fn answer_cursor_query<'a>(&self, chunk: &'a [u8]) -> &'a [u8] {
        match chunk.strip_prefix(CURSOR_QUERY) {
            Some(rest) => {
                let _ = self.input.lock().unwrap().write_all(CURSOR_AT_ORIGIN);
                rest
            }
            None => chunk,
        }
    }

    /// 설정 스크립트 뒤에 명령을 띄우고 붙은 데몬에 새 PID를 알린다. 데몬이 끝내기를 보냈으면 띄우지 않는다.
    fn replace(
        &self,
        slave: &dyn SlavePty,
        cmd: CommandBuilder,
    ) -> Result<Option<Box<dyn Child + Send + Sync>>, String> {
        let mut process = self.process.lock().unwrap();
        if process.killed {
            return Ok(None);
        }
        let child = slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        process.killer = child.clone_killer();
        let mut state = self.state.lock().unwrap();
        state.pid = child.process_id();
        if let Some(pid) = state.pid {
            state.send(&Frame::Pid(pid));
        }
        Ok(Some(child))
    }

    /// 프로세스가 아니라 호스트가 쓰는 한 줄. 화면이 터미널 출력으로 본다.
    fn note(&self, text: &str) {
        let line = format!("\r\n{text}\r\n").into_bytes();
        let mut state = self.state.lock().unwrap();
        state.backlog.push(&line);
        state.send(&Frame::Data(line));
    }

    fn exited(&self, exit: Exit) {
        let mut state = self.state.lock().unwrap();
        state.exit = Some(exit);
        if state.send(&exit.frame()) {
            let _ = self.delivered.send(());
        }
    }

    /// 데몬 연결 하나. 인사 뒤 backlog(와 끝났으면 종료 코드)를 보내고 그 연결을 지금 연결로 둔다.
    fn serve(&self, mut stream: TcpStream) {
        let greeted_pid = self.state.lock().unwrap().pid;
        let greeted = stream
            .set_read_timeout(Some(HELLO_TIMEOUT))
            .and_then(|()| wire::welcome(&mut stream, greeted_pid))
            .and_then(|()| stream.set_read_timeout(None));
        let Ok(mut reader) = greeted.and_then(|()| stream.try_clone()) else {
            return;
        };
        let me = {
            let mut state = self.state.lock().unwrap();
            let backlog = state.backlog.to_vec();
            // 인사한 뒤 설정 스크립트가 끝나 명령이 떴으면 새 PID를 먼저 알린다.
            let pid = state.pid.filter(|&pid| Some(pid) != greeted_pid);
            let sent = pid.is_none_or(|pid| wire::write(&mut stream, &Frame::Pid(pid)).is_ok())
                && (backlog.is_empty() || wire::write(&mut stream, &Frame::Data(backlog)).is_ok())
                && state
                    .exit
                    .is_none_or(|exit| wire::write(&mut stream, &exit.frame()).is_ok());
            if !sent {
                return;
            }
            if state.exit.is_some() {
                let _ = self.delivered.send(());
            }
            let me = state.next_client;
            state.next_client += 1;
            if let Some((_, old)) = state.client.replace((me, stream)) {
                let _ = old.shutdown(Shutdown::Both);
            }
            me
        };
        while let Ok(frame) = wire::read(&mut reader) {
            match frame {
                // 끝난 프로세스에 쓰면 실패한다. 데몬에 따로 알리지 않는다. `Exit`가 간다.
                Frame::Data(bytes) => {
                    let _ = self.input.lock().unwrap().write_all(&bytes);
                }
                Frame::Resize { cols, rows } => {
                    let size = PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    let _ = self.master.lock().unwrap().resize(size);
                }
                // 이미 끝난 프로세스면 실패한다. 할 일이 없다.
                Frame::Kill => {
                    let mut process = self.process.lock().unwrap();
                    process.killed = true;
                    let _ = process.killer.kill();
                }
                Frame::Hello { .. } | Frame::Exit(_) | Frame::Pid(_) | Frame::SetupFailed(_) => {}
            }
        }
        let mut state = self.state.lock().unwrap();
        if state.client.as_ref().is_some_and(|(id, _)| *id == me) {
            state.client = None;
        }
    }
}

impl State {
    /// 붙은 데몬에 보낸다. 보내지 못하면 그 연결을 놓는다. 보냈으면 참이다.
    fn send(&mut self, frame: &Frame) -> bool {
        let Some((_, stream)) = &mut self.client else {
            return false;
        };
        let sent = wire::write(stream, frame).is_ok();
        if !sent {
            self.client = None;
        }
        sent
    }
}
