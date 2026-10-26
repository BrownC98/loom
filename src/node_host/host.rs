use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};

use crate::tunnel::ENDPOINT_ENV;

use super::endpoint::Endpoint;
use super::inner::Inner;
use super::wire::{self, Frame, Link};
use super::{Backlog, Exit};

const INITIAL_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};
pub(super) const INITIAL_SIZE_VIEW: super::PtySize = super::PtySize {
    cols: INITIAL_SIZE.cols,
    rows: INITIAL_SIZE.rows,
};
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
const CURSOR_AT_ORIGIN: &[u8] = b"\x1b[1;1R";
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(1);
const EXIT_KEEP: Duration = Duration::from_secs(600);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

struct Host {
    process: Mutex<Process>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Box<dyn Write + Send>>,
    pid: Mutex<Option<u32>>,
    state: Mutex<State>,
    delivered: mpsc::Sender<()>,
    endpoint: Mutex<Option<Endpoint>>,
}

struct Process {
    killer: Box<dyn ChildKiller + Send + Sync>,
    killed: bool,
}

#[derive(Default)]
struct State {
    backlog: Backlog,
    client: Option<(u64, TcpStream)>,
    client_version: u32,
    next_client: u64,
    exit: Option<Exit>,
    endpoint_failed: Option<String>,
    closed: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Spec {
    argv: Vec<OsString>,
    cwd: Option<OsString>,
    env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    setup: Option<Vec<OsString>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<OsString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inner: Option<Inner>,
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
            endpoint: cmd.get_env(ENDPOINT_ENV).map(Into::into),
            inner: None,
        }
    }

    pub fn inner(
        launcher: &CommandBuilder,
        cmd: &CommandBuilder,
        setup: Option<&CommandBuilder>,
    ) -> Self {
        Self {
            inner: Some(Inner::of(cmd, setup)),
            endpoint: None,
            ..Self::of(launcher, None)
        }
    }

    fn process(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..])
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

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
    let mut spec: Spec =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("command: {e}"))?;
    if let Some(inner) = spec.inner.take() {
        return super::inner::outer(spec.process(), inner);
    }
    let endpoint = spec.endpoint.take();
    let (cmd, setup) = spec.commands();
    let started = start(cmd, setup, endpoint.as_deref())?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", started.address)
        .and_then(|()| stdout.flush())
        .map_err(|e| e.to_string())?;
    started.wait();
    Ok(())
}

pub struct Started {
    pub address: SocketAddr,
    host: Arc<Host>,
    running: Running,
    next: Option<CommandBuilder>,
    delivered: mpsc::Receiver<()>,
}

pub fn start(
    cmd: CommandBuilder,
    setup: Option<CommandBuilder>,
    endpoint: Option<&OsStr>,
) -> Result<Started, String> {
    let reported = Arc::new(OnceLock::<Weak<Host>>::new());
    let opened = endpoint.map(|at| {
        let to = reported.clone();
        let report = move |e: String| {
            if let Some(host) = to.get().and_then(Weak::upgrade) {
                host.endpoint_failed(e);
            }
        };
        Endpoint::open(at, report).map_err(|e| format!("{}: {e}", at.display()))
    });
    let pair = native_pty_system()
        .openpty(INITIAL_SIZE)
        .map_err(|e| e.to_string())?;
    let (first, next) = match setup {
        Some(setup) => (setup, Some(cmd)),
        None => (cmd, None),
    };
    let child = pair.slave.spawn_command(first).map_err(|e| e.to_string())?;
    drop(pair.slave);
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
        pid: Mutex::new(child.process_id()),
        state: Mutex::new(State {
            endpoint_failed: opened.as_ref().and_then(|o| o.as_ref().err()).cloned(),
            ..State::default()
        }),
        delivered: delivered_tx,
        endpoint: Mutex::new(opened.and_then(Result::ok)),
    });
    let _ = reported.set(Arc::downgrade(&host));
    let output_done = host.read_in_background(reader);
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
        running: Running { child, output_done },
        next,
        delivered,
    })
}

impl Started {
    pub fn stopper(&self) -> impl Fn() + Send + use<> {
        let host = self.host.clone();
        move || {
            host.kill();
            host.endpoint.lock().unwrap().take();
        }
    }

    pub fn wait(mut self) {
        let code = self.running.exit_code();
        let mut note = None;
        let exit = match self.next.take() {
            None => Exit::Command(code),
            Some(_) if code != 0 => Exit::Setup(code),
            Some(cmd) => {
                self.running.drain();
                match self.host.replace(cmd) {
                    Ok(Some(running)) => {
                        self.running = running;
                        Exit::Command(self.running.exit_code())
                    }
                    Ok(None) => Exit::Command(code),
                    Err(e) => {
                        note = Some(format!("loom: 에이전트를 띄우지 못했다: {e}"));
                        Exit::Command(u32::MAX)
                    }
                }
            }
        };
        self.running.drain();
        if let Some(note) = note {
            self.host.note(&note);
        }
        self.host.exited(exit);
        let _ = self.delivered.recv_timeout(EXIT_KEEP);
        self.host.close();
    }
}

struct Running {
    child: Box<dyn Child + Send + Sync>,
    output_done: mpsc::Receiver<()>,
}

impl Running {
    fn exit_code(&mut self) -> u32 {
        self.child.wait().map(|s| s.exit_code()).unwrap_or(u32::MAX)
    }

    fn drain(&self) {
        let _ = self.output_done.recv_timeout(DRAIN_AFTER_EXIT);
    }
}

impl Host {
    fn read_in_background(self: &Arc<Self>, reader: Box<dyn Read + Send>) -> mpsc::Receiver<()> {
        let (drained, output_done) = mpsc::channel::<()>();
        let pty = self.clone();
        std::thread::spawn(move || {
            pty.read_output(reader);
            drop(drained);
        });
        output_done
    }

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

    fn answer_cursor_query<'a>(&self, chunk: &'a [u8]) -> &'a [u8] {
        match chunk.strip_prefix(CURSOR_QUERY) {
            Some(rest) => {
                let _ = self.input.lock().unwrap().write_all(CURSOR_AT_ORIGIN);
                rest
            }
            None => chunk,
        }
    }

    fn replace(self: &Arc<Self>, cmd: CommandBuilder) -> Result<Option<Running>, String> {
        let mut process = self.process.lock().unwrap();
        if process.killed {
            return Ok(None);
        }
        let mut master = self.master.lock().unwrap();
        let size = master.get_size().unwrap_or(INITIAL_SIZE);
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| e.to_string())?;
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        *self.input.lock().unwrap() = pair.master.take_writer().map_err(|e| e.to_string())?;
        *master = pair.master;
        drop(master);
        process.killer = child.clone_killer();
        *self.pid.lock().unwrap() = child.process_id();
        if let Some(pid) = child.process_id() {
            self.state.lock().unwrap().send(&Frame::Pid(pid));
        }
        let output_done = self.read_in_background(reader);
        Ok(Some(Running { child, output_done }))
    }

    fn kill(&self) {
        let mut process = self.process.lock().unwrap();
        process.killed = true;
        let _ = process.killer.kill();
    }

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

    fn endpoint_failed(&self, error: String) {
        let mut state = self.state.lock().unwrap();
        if state.client_version >= wire::ENDPOINT_FAILED_SINCE {
            state.send(&Frame::EndpointFailed(error.clone()));
        }
        state.endpoint_failed = Some(error);
    }

    fn close(&self) {
        self.endpoint.lock().unwrap().take();
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        if let Some((_, client)) = state.client.take() {
            let _ = client.shutdown(Shutdown::Write);
        }
    }

    fn serve(&self, mut stream: TcpStream) {
        let greeted_pid = *self.pid.lock().unwrap();
        let greeted = stream
            .set_read_timeout(Some(HELLO_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT)))
            .and_then(|()| wire::welcome(&mut stream, greeted_pid));
        let version = match greeted {
            Ok(Link::Terminal { version }) => version,
            Ok(Link::Streams) => {
                match &*self.endpoint.lock().unwrap() {
                    Some(endpoint) => endpoint.carry(stream),
                    None => drop(wire::write(&mut stream, &Frame::Held(0))),
                }
                return;
            }
            Err(_) => return,
        };
        let Ok(mut reader) = stream
            .set_read_timeout(None)
            .and_then(|()| stream.try_clone())
        else {
            return;
        };
        let size = self
            .master
            .lock()
            .unwrap()
            .get_size()
            .unwrap_or(INITIAL_SIZE);
        if wire::write(
            &mut stream,
            &Frame::Resize {
                cols: size.cols,
                rows: size.rows,
            },
        )
        .is_err()
        {
            return;
        }
        let me = {
            let mut state = self.state.lock().unwrap();
            let backlog = state.backlog.to_vec();
            let pid = self
                .pid
                .lock()
                .unwrap()
                .filter(|&pid| Some(pid) != greeted_pid);
            let sent = pid.is_none_or(|pid| wire::write(&mut stream, &Frame::Pid(pid)).is_ok())
                && (backlog.is_empty() || wire::write(&mut stream, &Frame::Data(backlog)).is_ok())
                && (version < wire::ENDPOINT_FAILED_SINCE
                    || state.endpoint_failed.as_ref().is_none_or(|error| {
                        wire::write(&mut stream, &Frame::EndpointFailed(error.clone())).is_ok()
                    }))
                && state
                    .exit
                    .is_none_or(|exit| wire::write(&mut stream, &exit.frame()).is_ok());
            if !sent || state.closed {
                return;
            }
            if state.exit.is_some() {
                let _ = self.delivered.send(());
            }
            let me = state.next_client;
            state.next_client += 1;
            state.client_version = version;
            if let Some((_, old)) = state.client.replace((me, stream)) {
                let _ = old.shutdown(Shutdown::Both);
            }
            me
        };
        while let Ok(frame) = wire::read(&mut reader) {
            match frame {
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
                Frame::Kill => self.kill(),
                Frame::Hello { .. }
                | Frame::Exit(_)
                | Frame::Pid(_)
                | Frame::SetupFailed(_)
                | Frame::Streams { .. }
                | Frame::EndpointFailed(_)
                | Frame::Held(_) => {}
            }
        }
        let mut state = self.state.lock().unwrap();
        if state.client.as_ref().is_some_and(|(id, _)| *id == me) {
            state.client = None;
        }
    }
}

impl State {
    fn send(&mut self, frame: &Frame) -> bool {
        let Some((_, stream)) = &mut self.client else {
            return false;
        };
        let sent = wire::write(stream, frame).is_ok();
        if !sent && let Some((_, stream)) = self.client.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        sent
    }
}
