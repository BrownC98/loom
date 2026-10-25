use std::fmt::Debug;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use ureq::Error;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, LazyBuffers, NextTimeout, Transport,
};

pub const URL: &str = "http://127.0.0.1";

#[derive(Debug)]
pub struct Connector(pub PathBuf);

impl ureq::unversioned::transport::Connector for Connector {
    type Out = Socket;

    fn connect(&self, details: &ConnectionDetails, _: Option<()>) -> Result<Option<Socket>, Error> {
        let wire = open(&self.0).map_err(|_| Error::ConnectionFailed)?;
        let config = details.config;
        Ok(Some(Socket {
            wire,
            buffers: LazyBuffers::new(config.input_buffer_size(), config.output_buffer_size()),
        }))
    }
}

trait Wire: Read + Write + Send + Sync + Debug + 'static {
    fn limit(&self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()>;
}

#[cfg(unix)]
fn open(at: &std::path::Path) -> io::Result<Box<dyn Wire>> {
    Ok(Box::new(std::os::unix::net::UnixStream::connect(at)?))
}

#[cfg(unix)]
impl Wire for std::os::unix::net::UnixStream {
    fn limit(&self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(read)?;
        self.set_write_timeout(write)
    }
}

#[cfg(windows)]
fn open(at: &std::path::Path) -> io::Result<Box<dyn Wire>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Pipes::{NMPWAIT_WAIT_FOREVER, WaitNamedPipeW};
    const PIPE_BUSY: i32 = 231;
    loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(at) {
            Err(e) if e.raw_os_error() == Some(PIPE_BUSY) => {
                let name: Vec<u16> = at.as_os_str().encode_wide().chain([0]).collect();
                if unsafe { WaitNamedPipeW(name.as_ptr(), NMPWAIT_WAIT_FOREVER) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            opened => return Ok(Box::new(opened?)),
        }
    }
}

#[cfg(windows)]
impl Wire for std::fs::File {
    fn limit(&self, _: Option<Duration>, _: Option<Duration>) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct Socket {
    wire: Box<dyn Wire>,
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
        self.wire.limit(None, limit(&timeout))?;
        let output = &self.buffers.output()[..amount];
        self.wire
            .write_all(output)
            .map_err(|e| timed_out(e, &timeout))
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        self.wire.limit(limit(&timeout), None)?;
        let input = self.buffers.input_append_buf();
        let amount = self.wire.read(input).map_err(|e| timed_out(e, &timeout))?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        false
    }
}
