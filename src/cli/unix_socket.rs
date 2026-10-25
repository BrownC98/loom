use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ureq::Error;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, LazyBuffers, NextTimeout, Transport,
};

pub const URL: &str = "http://127.0.0.1";
pub const RESTART_WAIT: Duration = Duration::from_secs(10);
pub const PROBE_WAIT: Duration = Duration::from_secs(2);
const CONNECT_AGAIN_AFTER: Duration = Duration::from_millis(200);

fn restarting(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

pub fn connect(sock: &Path, wait: Duration) -> io::Result<UnixStream> {
    let until = Instant::now() + wait;
    loop {
        match UnixStream::connect(sock) {
            Err(e) if restarting(&e) && Instant::now() < until => std::thread::sleep(
                CONNECT_AGAIN_AFTER.min(until.saturating_duration_since(Instant::now())),
            ),
            connected => return connected,
        }
    }
}

#[derive(Debug)]
pub struct Connector(pub PathBuf);

impl ureq::unversioned::transport::Connector for Connector {
    type Out = Socket;

    fn connect(&self, details: &ConnectionDetails, _: Option<()>) -> Result<Option<Socket>, Error> {
        let budget = limit(&details.timeout).filter(|b| *b < RESTART_WAIT);
        let stream =
            connect(&self.0, budget.unwrap_or(RESTART_WAIT)).map_err(|e| match budget {
                Some(_) if restarting(&e) => Error::Timeout(details.timeout.reason),
                _ => Error::ConnectionFailed,
            })?;
        let config = details.config;
        Ok(Some(Socket {
            stream,
            buffers: LazyBuffers::new(config.input_buffer_size(), config.output_buffer_size()),
        }))
    }
}

#[derive(Debug)]
pub struct Socket {
    stream: UnixStream,
    buffers: LazyBuffers,
}

fn timed_out(e: io::Error, timeout: &NextTimeout) -> Error {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Error::Timeout(timeout.reason),
        _ => e.into(),
    }
}

fn limit(timeout: &NextTimeout) -> Option<Duration> {
    timeout.not_zero().map(|t| *t)
}

impl Transport for Socket {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.stream.set_write_timeout(limit(&timeout))?;
        let output = &self.buffers.output()[..amount];
        self.stream
            .write_all(output)
            .map_err(|e| timed_out(e, &timeout))
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        self.stream.set_read_timeout(limit(&timeout))?;
        let input = self.buffers.input_append_buf();
        let amount = self
            .stream
            .read(input)
            .map_err(|e| timed_out(e, &timeout))?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        false
    }
}
