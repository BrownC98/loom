use std::fmt::Debug;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ureq::Error;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, LazyBuffers, NextTimeout, Transport,
};

pub const URL: &str = "http://127.0.0.1";

#[derive(Debug)]
pub struct Connector {
    pub at: PathBuf,
    pub close: Option<Close>,
}

impl ureq::unversioned::transport::Connector for Connector {
    type Out = Socket;

    fn connect(&self, details: &ConnectionDetails, _: Option<()>) -> Result<Option<Socket>, Error> {
        let wire = open(&self.at).map_err(|_| Error::ConnectionFailed)?;
        if let Some(close) = &self.close {
            close.hold(wire.as_ref())?;
        }
        let config = details.config;
        Ok(Some(Socket {
            wire,
            buffers: LazyBuffers::new(config.input_buffer_size(), config.output_buffer_size()),
        }))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Close(Arc<Mutex<Shut>>);

#[derive(Debug, Default)]
struct Shut {
    closed: bool,
    wire: Option<Shutter>,
}

impl Close {
    pub fn close(&self) {
        let mut shut = self.0.lock().unwrap();
        shut.closed = true;
        if let Some(wire) = &shut.wire {
            wire.shut();
        }
    }

    pub fn closed(&self) -> bool {
        self.0.lock().unwrap().closed
    }

    fn hold(&self, wire: &dyn Wire) -> io::Result<()> {
        let mut shut = self.0.lock().unwrap();
        if shut.closed {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        shut.wire = Some(wire.shutter()?);
        Ok(())
    }
}

trait Wire: Read + Write + Send + Sync + Debug + 'static {
    fn limit(&mut self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()>;
    fn shutter(&self) -> io::Result<Shutter>;
}

#[cfg(unix)]
fn open(at: &std::path::Path) -> io::Result<Box<dyn Wire>> {
    Ok(Box::new(std::os::unix::net::UnixStream::connect(at)?))
}

#[cfg(unix)]
#[derive(Debug)]
struct Shutter(std::os::unix::net::UnixStream);

#[cfg(unix)]
impl Shutter {
    fn shut(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(unix)]
impl Wire for std::os::unix::net::UnixStream {
    fn limit(&mut self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(read)?;
        self.set_write_timeout(write)
    }

    fn shutter(&self) -> io::Result<Shutter> {
        self.try_clone().map(Shutter)
    }
}

#[cfg(windows)]
use pipe::Shutter;

#[cfg(windows)]
fn open(at: &std::path::Path) -> io::Result<Box<dyn Wire>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
    let file = crate::tunnel::open_pipe(
        at,
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED),
    )?;
    Ok(Box::new(pipe::Pipe {
        file: Arc::new(file),
        read: None,
        write: None,
        shut: Arc::default(),
    }))
}

#[cfg(windows)]
mod pipe {
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, HANDLE, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::IO::{
        CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
    };
    use windows_sys::Win32::System::Threading::INFINITE;
    use windows_sys::core::BOOL;

    #[derive(Debug)]
    pub struct Pipe {
        pub file: Arc<std::fs::File>,
        pub read: Option<Duration>,
        pub write: Option<Duration>,
        pub shut: Arc<Mutex<bool>>,
    }

    #[derive(Debug)]
    pub struct Shutter {
        file: Arc<std::fs::File>,
        shut: Arc<Mutex<bool>>,
    }

    impl Shutter {
        pub fn shut(&self) {
            let mut shut = self.shut.lock().unwrap();
            *shut = true;
            unsafe { CancelIoEx(self.file.as_raw_handle(), std::ptr::null()) };
        }
    }

    impl Pipe {
        fn within(
            &self,
            limit: Option<Duration>,
            start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> BOOL,
        ) -> io::Result<usize> {
            let handle = self.file.as_raw_handle();
            let mut overlapped = OVERLAPPED::default();
            let shut = self.shut.lock().unwrap();
            if *shut {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if start(handle, &mut overlapped) == 0 {
                let e = io::Error::last_os_error();
                if e.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                    return Err(e);
                }
            }
            drop(shut);
            let wait = limit.map_or(INFINITE, |limit| {
                u32::try_from(limit.as_millis()).map_or(INFINITE - 1, |ms| ms.min(INFINITE - 1))
            });
            let mut done = 0;
            if unsafe { GetOverlappedResultEx(handle, &overlapped, &mut done, wait, 0) } != 0 {
                return Ok(done as usize);
            }
            let e = io::Error::last_os_error();
            if ![WAIT_TIMEOUT, ERROR_IO_INCOMPLETE]
                .map(|c| Some(c as i32))
                .contains(&e.raw_os_error())
            {
                return Err(e);
            }
            unsafe { CancelIoEx(handle, &overlapped) };
            match unsafe { GetOverlappedResult(handle, &overlapped, &mut done, 1) } {
                0 => Err(io::ErrorKind::TimedOut.into()),
                _ => Ok(done as usize),
            }
        }
    }

    impl super::Wire for Pipe {
        fn limit(&mut self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()> {
            self.read = read;
            self.write = write;
            Ok(())
        }

        fn shutter(&self) -> io::Result<Shutter> {
            Ok(Shutter {
                file: self.file.clone(),
                shut: self.shut.clone(),
            })
        }
    }

    fn len(buf: &[u8]) -> u32 {
        u32::try_from(buf.len()).unwrap_or(u32::MAX)
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = self.within(self.read, |handle, overlapped| unsafe {
                ReadFile(
                    handle,
                    buf.as_mut_ptr(),
                    len(buf),
                    std::ptr::null_mut(),
                    overlapped,
                )
            });
            match read {
                Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
                read => read,
            }
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.within(self.write, |handle, overlapped| unsafe {
                WriteFile(
                    handle,
                    buf.as_ptr(),
                    len(buf),
                    std::ptr::null_mut(),
                    overlapped,
                )
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
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
