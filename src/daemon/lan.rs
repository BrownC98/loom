use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use base64::Engine;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use crate::devices::Devices;
use crate::pake;

const PORT_OFFSET: u16 = 10;
const HANDSHAKE: Duration = Duration::from_secs(10);
const BACKLOG: usize = 64;
pub const FIND_PORT: u16 = 7430;
const FIND_MAX: usize = 2048;
const CERT_FILE: &str = "phone-tls.crt";
const KEY_FILE: &str = "phone-tls.key";
const VIRTUAL: &[&str] = &[
    "docker",
    "br-",
    "virbr",
    "vmnet",
    "vboxnet",
    "veth",
    "lxcbr",
    "cni",
    "flannel",
    "cali",
    "bridge",
    "vethernet",
    "vmware",
    "virtualbox",
];

#[derive(Clone, Copy)]
pub struct Lan;

#[derive(Clone)]
pub struct Opened {
    pub port: u16,
    pub fingerprint: String,
}

impl Opened {
    pub fn door(&self, addresses: &[Ipv4Addr]) -> String {
        let doors: Vec<String> = addresses
            .iter()
            .map(|ip| format!("{ip}:{}", self.port))
            .collect();
        format!("fp={}&a={}", self.fingerprint, doors.join(","))
    }
}

pub struct Door {
    data: PathBuf,
    daemon_port: u16,
    devices: Arc<Devices>,
    router: OnceLock<Router>,
    opened: tokio::sync::Mutex<Option<Opened>>,
}

impl Door {
    pub fn new(data: PathBuf, daemon_port: u16, devices: Arc<Devices>) -> Self {
        Self {
            data,
            daemon_port,
            devices,
            router: OnceLock::new(),
            opened: tokio::sync::Mutex::default(),
        }
    }

    pub fn set_router(&self, router: Router) {
        let _ = self.router.set(router);
    }

    pub async fn open(&self) -> std::io::Result<Opened> {
        let mut opened = self.opened.lock().await;
        if let Some(open) = &*opened {
            return Ok(open.clone());
        }
        let router = self
            .router
            .get()
            .cloned()
            .ok_or_else(|| std::io::Error::other("phone door has no router yet"))?;
        let (config, fingerprint) = server_config(&self.data)?;
        let preferred = self.daemon_port.checked_add(PORT_OFFSET).unwrap_or(0);
        let tcp = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, preferred)).await {
            Ok(tcp) => tcp,
            Err(_) => TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?,
        };
        let open = Opened {
            port: tcp.local_addr()?.port(),
            fingerprint,
        };
        let listener = TlsListener::start(tcp, TlsAcceptor::from(Arc::new(config)))?;
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router.layer(axum::Extension(Lan))).await {
                crate::log::logln!("loom: phone door stopped: {e}");
            }
        });
        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, FIND_PORT)).await {
            Ok(socket) => {
                tokio::spawn(answer_finds(socket, self.devices.clone(), open.clone()));
            }
            Err(e) => crate::log::logln!("loom: phone find port {FIND_PORT}: {e}"),
        }
        *opened = Some(open.clone());
        Ok(open)
    }

    pub async fn current(&self) -> Option<Opened> {
        self.opened.lock().await.clone()
    }
}

#[derive(Serialize, Deserialize)]
struct Find {
    pa: String,
}

#[derive(Serialize, Deserialize)]
struct Found {
    door: String,
    pb: String,
    c: String,
}

async fn answer_finds(socket: UdpSocket, devices: Arc<Devices>, open: Opened) {
    let b64 = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut buf = vec![0u8; FIND_MAX];
    loop {
        let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Some(pa) = serde_json::from_slice::<Find>(&buf[..len])
            .ok()
            .and_then(|find| b64.decode(find.pa).ok())
        else {
            continue;
        };
        let door = open.door(&addresses());
        let devices = devices.clone();
        let answered = tokio::task::spawn_blocking(move || {
            let pa = pake::Element::parse(&pa)?;
            let answer = devices.answer(&pa, door.as_bytes())?;
            Some(Found {
                door,
                pb: b64.encode(answer.pb),
                c: b64.encode(answer.confirm),
            })
        })
        .await;
        if let Ok(Some(found)) = answered {
            let reply = serde_json::to_vec(&found).expect("글만 있는 구조체");
            let _ = socket.send_to(&reply, peer).await;
        }
    }
}

const FIREWALL_RULE: &str = "loom-phone";
const FIREWALL_FIND_RULE: &str = "loom-phone-find";

pub async fn allow_firewall(port: u16) -> std::io::Result<()> {
    if !cfg!(windows) {
        return Err(std::io::Error::other(
            "the firewall rule is only for Windows",
        ));
    }
    let exe = ps_quote(&std::env::current_exe()?.to_string_lossy());
    let rule = ps_quote(FIREWALL_RULE);
    let find_rule = ps_quote(FIREWALL_FIND_RULE);
    let inner = format!(
        "$ErrorActionPreference = 'Stop'\n\
         Get-NetFirewallApplicationFilter -Program {exe} -ErrorAction SilentlyContinue | Get-NetFirewallRule | \
         Where-Object {{ $_.Direction -eq 'Inbound' -and $_.Action -eq 'Block' }} | Remove-NetFirewallRule\n\
         Get-NetFirewallRule -Name {rule},{find_rule} -ErrorAction SilentlyContinue | Remove-NetFirewallRule\n\
         New-NetFirewallRule -Name {rule} -DisplayName 'loom phone' -Direction Inbound -Action Allow -Profile Private \
         -Protocol TCP -LocalPort {port} -Program {exe} -EdgeTraversalPolicy Block | Out-Null\n\
         New-NetFirewallRule -Name {find_rule} -DisplayName 'loom phone find' -Direction Inbound -Action Allow -Profile Private \
         -Protocol UDP -LocalPort {FIND_PORT} -Program {exe} -EdgeTraversalPolicy Block | Out-Null"
    );
    let encoded: String = base64::engine::general_purpose::STANDARD.encode(
        inner
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<u8>>(),
    );
    let outer = format!(
        "$p = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand','{encoded}') \
         -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode"
    );
    let status = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .status()
        .await?;
    match status.success() {
        true => Ok(()),
        false => Err(std::io::Error::other(format!(
            "firewall rule failed: {status}"
        ))),
    }
}

fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

pub fn addresses() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut found: Vec<Ipv4Addr> = interfaces
        .iter()
        .filter(|i| {
            i.is_oper_up() && !i.is_loopback() && i.name != "lo" && !virtual_adapter(&i.name)
        })
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() || tailnet(ip) => Some(ip),
            _ => None,
        })
        .collect();
    found.sort_by_key(|ip| (tailnet(*ip), *ip));
    found.dedup();
    found
}

pub fn tailnet(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

fn virtual_adapter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    VIRTUAL
        .iter()
        .any(|v| name.starts_with(v) || (v.len() > 4 && name.contains(v)))
}

fn server_config(data: &Path) -> std::io::Result<(rustls::ServerConfig, String)> {
    let (cert, key) = match read_pair(data) {
        Some(pair) => pair,
        None => make_pair(data)?,
    };
    let fingerprint = fingerprint(&cert);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(std::io::Error::other)?;
    Ok((config, fingerprint))
}

fn read_pair(data: &Path) -> Option<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let cert = std::fs::read(data.join(CERT_FILE)).ok()?;
    let key = std::fs::read(data.join(KEY_FILE)).ok()?;
    Some((
        CertificateDer::from(cert),
        PrivatePkcs8KeyDer::from(key).into(),
    ))
}

fn make_pair(data: &Path) -> std::io::Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let made = rcgen::generate_simple_self_signed(vec!["loom".to_owned()])
        .map_err(std::io::Error::other)?;
    let cert = made.cert.der().clone();
    let key = made.signing_key.serialize_der();
    std::fs::create_dir_all(data)?;
    write_secret(&data.join(KEY_FILE), &key)?;
    std::fs::write(data.join(CERT_FILE), &cert)?;
    Ok((cert, PrivatePkcs8KeyDer::from(key).into()))
}

#[cfg(unix)]
fn write_secret(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
fn write_secret(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn fingerprint(cert: &CertificateDer<'_>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(cert.as_ref()))
}

struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    addr: SocketAddr,
}

impl TlsListener {
    fn start(tcp: TcpListener, acceptor: TlsAcceptor) -> std::io::Result<Self> {
        let addr = tcp.local_addr()?;
        let (tx, ready) = mpsc::channel(BACKLOG);
        tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = tcp.accept().await else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                let acceptor = acceptor.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Ok(Ok(tls)) =
                        tokio::time::timeout(HANDSHAKE, acceptor.accept(stream)).await
                    {
                        let _ = tx.send((tls, peer)).await;
                    }
                });
            }
        });
        Ok(Self { ready, addr })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(conn) => conn,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}
