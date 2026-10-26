use std::ffi::{OsStr, OsString};
use std::io;
use std::net::TcpStream;

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::wire::{self, Frame};
use crate::tunnel::{self, Mux};

pub struct Endpoint {
    links: mpsc::UnboundedSender<TcpStream>,
    #[cfg(unix)]
    _bound: Bound,
}

type Opened = (Mux, mpsc::UnboundedReceiver<tunnel::Stream>);

impl Endpoint {
    pub fn open(at: &OsStr, failed: impl Fn(String) + Send + 'static) -> io::Result<Endpoint> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()?;
        let listener = {
            let _entered = runtime.enter();
            Listener::bind(at)?
        };
        #[cfg(unix)]
        let bound = listener.bound()?;
        let (links, waiting) = mpsc::unbounded_channel();
        std::thread::spawn(move || runtime.block_on(serve(listener, waiting, failed)));
        Ok(Endpoint {
            links,
            #[cfg(unix)]
            _bound: bound,
        })
    }

    pub fn carry(&self, link: TcpStream) {
        let _ = self.links.send(link);
    }
}

async fn serve(
    mut listener: Listener,
    mut links: mpsc::UnboundedReceiver<TcpStream>,
    failed: impl Fn(String),
) {
    let mut mux: Option<Opened> = None;
    let mut held = Vec::new();
    let mut carried = JoinSet::new();
    let mut accepting = true;
    loop {
        tokio::select! {
            accepted = listener.accept(), if accepting => match accepted {
                Ok(conn) => match &mux {
                    Some((open, _)) if !open.is_closed() => {
                        carried.spawn(tunnel::splice(conn, open.open()));
                    }
                    _ => {
                        mux = None;
                        held.push(conn);
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::ConnectionAborted => {}
                Err(e) => {
                    accepting = false;
                    if carried.is_empty() {
                        failed(format!("accept: {e}"));
                    }
                }
            },
            Some(_) = carried.join_next() => accepting = true,
            link = links.recv() => match link.map(|link| attach(link, held.len())) {
                Some(Ok((opener, events))) => {
                    for conn in held.drain(..) {
                        carried.spawn(tunnel::splice(conn, opener.open()));
                    }
                    mux = Some((opener, events));
                    accepting = true;
                }
                Some(Err(e)) => failed(format!("the daemon link: {e}")),
                None => return,
            },
            () = link_ended(&mut mux) => mux = None,
        }
    }
}

async fn link_ended(mux: &mut Option<Opened>) {
    match mux {
        Some((_, events)) => while events.recv().await.is_some() {},
        None => std::future::pending().await,
    }
}

fn attach(mut link: TcpStream, held: usize) -> io::Result<Opened> {
    wire::write(&mut link, &Frame::Held(held as u32))?;
    link.set_read_timeout(None)?;
    link.set_write_timeout(None)?;
    link.set_nonblocking(true)?;
    let (reader, writer) = tokio::net::TcpStream::from_std(link)?.into_split();
    Ok(tunnel::start(reader, writer))
}

#[cfg(unix)]
struct Listener(tokio::net::UnixListener, OsString);

#[cfg(unix)]
impl Listener {
    fn bind(at: &OsStr) -> io::Result<Listener> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let sock = std::path::Path::new(at);
        let dir = sock
            .parent()
            .ok_or_else(|| io::Error::other("소켓 경로에 폴더가 없다"))?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let listener = match tokio::net::UnixListener::bind(sock) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                if std::os::unix::net::UnixStream::connect(sock).is_ok() {
                    return Err(e);
                }
                std::fs::remove_file(sock)?;
                tokio::net::UnixListener::bind(sock)?
            }
            bound => bound?,
        };
        Ok(Listener(listener, at.into()))
    }

    fn bound(&self) -> io::Result<Bound> {
        use std::os::unix::fs::MetadataExt;
        Ok(Bound {
            ino: std::fs::metadata(&self.1)?.ino(),
            path: self.1.clone(),
        })
    }

    async fn accept(&mut self) -> io::Result<tokio::net::UnixStream> {
        self.0.accept().await.map(|(conn, _)| conn)
    }
}

#[cfg(unix)]
struct Bound {
    ino: u64,
    path: OsString,
}

#[cfg(unix)]
impl Drop for Bound {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.ino() == self.ino) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(windows)]
struct Listener {
    name: OsString,
    next: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
}

#[cfg(windows)]
impl Listener {
    fn bind(at: &OsStr) -> io::Result<Listener> {
        Ok(Listener {
            name: at.into(),
            next: Some(pipe::create(at, true)?),
        })
    }

    async fn accept(&mut self) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        let next = match &mut self.next {
            Some(next) => next,
            None => self.next.insert(pipe::create(&self.name, false)?),
        };
        let connected = next.connect().await;
        let server = self.next.take().expect("connect가 이 인스턴스를 기다렸다");
        self.next = pipe::create(&self.name, false).ok();
        connected.map(|()| server)
    }
}

#[cfg(windows)]
mod pipe {
    use std::ffi::OsStr;
    use std::io;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub fn create(name: &OsStr, first: bool) -> io::Result<NamedPipeServer> {
        let descriptor = Descriptor::user_only()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, (&raw mut attributes).cast())
        }
    }

    struct Descriptor(PSECURITY_DESCRIPTOR);

    impl Descriptor {
        fn user_only() -> io::Result<Descriptor> {
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", user_sid()?)
                .encode_utf16()
                .chain([0])
                .collect();
            let mut descriptor = std::ptr::null_mut();
            let made = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            match made {
                0 => Err(io::Error::last_os_error()),
                _ => Ok(Descriptor(descriptor)),
            }
        }
    }

    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0) };
        }
    }

    fn user_sid() -> io::Result<String> {
        let mut token: HANDLE = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u64; 64];
        let mut len = 0;
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut len,
            )
        };
        let read = match read {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        };
        unsafe { CloseHandle(token) };
        read?;
        let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
        let mut text = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
        };
        unsafe { LocalFree(text.cast()) };
        Ok(sid)
    }
}
