use std::ffi::{OsStr, OsString};
use std::io;
use std::net::TcpStream;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::tunnel::{self, Mux};

pub struct Endpoint {
    links: mpsc::UnboundedSender<TcpStream>,
    #[cfg(unix)]
    _bound: Bound,
}

impl Endpoint {
    pub fn open(at: &OsStr) -> io::Result<Endpoint> {
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
        std::thread::spawn(move || runtime.block_on(serve(listener, waiting)));
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

async fn serve(mut listener: Listener, mut links: mpsc::UnboundedReceiver<TcpStream>) {
    let mut mux: Option<(Mux, mpsc::UnboundedReceiver<tunnel::Stream>)> = None;
    let mut held = Vec::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(conn) => match &mux {
                    Some((open, _)) if !open.is_closed() => carry(conn, open),
                    _ => {
                        mux = None;
                        held.push(conn);
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::ConnectionAborted => {}
                Err(e) => {
                    eprintln!("loom: 노드 접속점이 연결을 받지 못해 닫는다: {e}");
                    return;
                }
            },
            Some(link) = links.recv() => match attach(link) {
                Ok((opener, events)) => {
                    for conn in held.drain(..) {
                        carry(conn, &opener);
                    }
                    mux = Some((opener, events));
                }
                Err(e) => eprintln!("loom: 데몬의 스트림 연결을 쓰지 못했다: {e}"),
            },
            () = link_ended(&mut mux) => mux = None,
        }
    }
}

async fn link_ended(mux: &mut Option<(Mux, mpsc::UnboundedReceiver<tunnel::Stream>)>) {
    match mux {
        Some((_, events)) => while events.recv().await.is_some() {},
        None => std::future::pending().await,
    }
}

fn attach(link: TcpStream) -> io::Result<(Mux, mpsc::UnboundedReceiver<tunnel::Stream>)> {
    link.set_read_timeout(None)?;
    link.set_write_timeout(None)?;
    link.set_nonblocking(true)?;
    let (reader, writer) = tokio::net::TcpStream::from_std(link)?.into_split();
    Ok(tunnel::start(reader, writer))
}

pub(super) fn carry<C>(conn: C, mux: &Mux)
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut from_client, mut to_client) = tokio::io::split(conn);
    let (mut from_daemon, mut to_daemon) = tokio::io::split(mux.open());
    tokio::spawn(async move {
        let request = async {
            let _ = tokio::io::copy(&mut from_client, &mut to_daemon).await;
            let _ = to_daemon.shutdown().await;
            std::future::pending::<()>().await
        };
        let answer = async {
            let _ = tokio::io::copy(&mut from_daemon, &mut to_client).await;
        };
        tokio::select! {
            () = request => {}
            () = answer => {}
        }
    });
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
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(windows)]
impl Listener {
    fn bind(at: &OsStr) -> io::Result<Listener> {
        let next = pipe::create(at, true)?;
        Ok(Listener {
            name: at.into(),
            next,
        })
    }

    async fn accept(&mut self) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        self.next.connect().await?;
        let next = pipe::create(&self.name, false)?;
        Ok(std::mem::replace(&mut self.next, next))
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
