use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr};

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use super::endpoint::carry;
use super::host;
use super::wire::VERSION;
use crate::tunnel::{self, ENDPOINT_ENV};

#[derive(Serialize, Deserialize)]
pub struct Inner {
    version: u32,
    argv: Vec<String>,
    setup: Option<Vec<String>>,
    endpoint: Option<String>,
}

impl Inner {
    pub fn of(cmd: &CommandBuilder, setup: Option<&CommandBuilder>) -> Inner {
        let argv = |cmd: &CommandBuilder| {
            cmd.get_argv()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        Inner {
            version: VERSION,
            argv: argv(cmd),
            setup: setup.map(argv),
            endpoint: cmd
                .get_env(ENDPOINT_ENV)
                .map(|at| at.to_string_lossy().into_owned()),
        }
    }
}

pub fn outer(mut launcher: tokio::process::Command, inner: Inner) -> Result<(), String> {
    let runtime = runtime()?;
    runtime.block_on(async move {
        launcher
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(false);
        #[cfg(windows)]
        launcher.creation_flags(0x0800_0000);
        let mut child = launcher
            .spawn()
            .map_err(|e| format!("inner pty-host: {e}"))?;
        let mut to_inner = child.stdin.take().expect("stdin을 piped로 열었다");
        let from_inner = child.stdout.take().expect("stdout을 piped로 열었다");
        let mut line = serde_json::to_vec(&inner).map_err(|e| e.to_string())?;
        line.push(b'\n');
        to_inner
            .write_all(&line)
            .await
            .map_err(|e| format!("inner pty-host: {e}"))?;
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| e.to_string())?;
        announce(listener.local_addr().map_err(|e| e.to_string())?)?;
        carry_out(listener, from_inner, to_inner).await;
        let _ = child.wait().await;
        Ok(())
    })
}

pub(super) async fn carry_out<R, W>(listener: tokio::net::TcpListener, from_inner: R, to_inner: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mux, mut events) = tunnel::start(from_inner, to_inner);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((conn, _)) = accepted {
                    carry(conn, &mux);
                }
            }
            event = events.recv() => if event.is_none() {
                return;
            },
        }
    }
}

pub fn inner() -> Result<(), String> {
    let spec: Inner =
        serde_json::from_slice(&first_line()?).map_err(|e| format!("command: {e}"))?;
    if spec.version != VERSION {
        return Err(format!(
            "the outer pty-host speaks version {}, this one {VERSION}",
            spec.version
        ));
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let command = |argv: Vec<String>| {
        let mut cmd = CommandBuilder::from_argv(argv.into_iter().map(Into::into).collect());
        cmd.cwd(&here);
        cmd
    };
    let endpoint = spec.endpoint.map(std::ffi::OsString::from);
    let started = host::start(
        command(spec.argv),
        spec.setup.map(command),
        endpoint.as_deref(),
    )?;
    let (address, stop) = (started.address, started.stopper());
    let (ended, wait_end) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        started.wait();
        let _ = ended.send(());
    });
    runtime()?.block_on(async move {
        tokio::select! {
            () = carry_in(address, tokio::io::stdin(), tokio::io::stdout()) => stop(),
            _ = wait_end => {}
        }
    });
    Ok(())
}

pub(super) async fn carry_in<R, W>(address: SocketAddr, from_outer: R, to_outer: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut events = tunnel::accept(from_outer, to_outer);
    while let Some(stream) = events.recv().await {
        tokio::spawn(to_host(address, stream));
    }
}

async fn to_host(address: SocketAddr, mut stream: tunnel::Stream) {
    if let Ok(mut conn) = tokio::net::TcpStream::connect(address).await {
        let _ = tokio::io::copy_bidirectional(&mut conn, &mut stream).await;
    }
}

fn first_line() -> Result<Vec<u8>, String> {
    let mut stdin = std::io::stdin().lock();
    let mut line = Vec::new();
    let mut byte = [0u8];
    loop {
        match stdin.read(&mut byte) {
            Ok(0) => return Err("the outer pty-host closed before its command".into()),
            Ok(_) if byte[0] == b'\n' => return Ok(line),
            Ok(_) => line.push(byte[0]),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn announce(address: SocketAddr) -> Result<(), String> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{address}")
        .and_then(|()| stdout.flush())
        .map_err(|e| e.to_string())
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .map_err(|e| e.to_string())
}
