//! 폰 문: Tailscale 없이 같은 네트워크의 Android 앱이 닿는 HTTPS 문. 근거: docs/design.md 「원격」, 목업 docs/mockups/phone-direct.html.
//!
//! 사람 문(PC 화면)은 루프백에 그대로 둔다. 폰 문은 모든 IPv4 주소에서 듣고, 데몬이 처음 한 번 만든 자체 서명 인증서를 쓴다.
//! 브라우저는 이 인증서를 믿지 않는다. 앱은 QR에 담은 인증서 지문(SHA-256)과 같을 때만 연다(Orca가 QR의 PC 공개 키로 확인하는 것과 같다).
//! 문을 지나는 요청은 모두 원격이다(`origin::human_site`).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use base64::Engine;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// 폰 문 포트는 데몬 포트에서 이만큼 떨어진다. 앱이 저장한 주소가 데몬을 다시 띄워도 맞게 늘 같은 포트를 먼저 쓴다.
const PORT_OFFSET: u16 = 10;
/// TLS 악수를 기다리는 시간. 악수하지 않고 붙잡는 연결이 자원을 쥐지 않게 한다.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// 악수를 마친 연결을 axum에 넘기는 줄의 길이.
const BACKLOG: usize = 64;
const CERT_FILE: &str = "phone-tls.crt";
const KEY_FILE: &str = "phone-tls.key";
/// 가상 어댑터. 폰이 닿지 않는 주소를 QR에 넣지 않는다. Orca의 같은 표(`docker|br-|virbr|…`, `vEthernet`)를 따른다.
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

/// 폰 문을 지난 요청에 문이 넣는 표시.
#[derive(Clone, Copy)]
pub struct Lan;

/// 연 폰 문. QR이 담는 값이다.
#[derive(Clone)]
pub struct Opened {
    pub port: u16,
    /// 인증서 DER의 SHA-256(base64url, 덧붙임 없음).
    pub fingerprint: String,
}

/// 폰 문은 처음 필요할 때 연다: 짝짓기 코드를 만들 때, 또는 짝지은 기기가 있는 채로 데몬이 시작할 때.
/// 폰을 쓰지 않는 사람의 데몬은 LAN에서 듣지 않는다(Windows 방화벽 창도 뜨지 않는다).
pub struct Door {
    data: PathBuf,
    daemon_port: u16,
    router: OnceLock<Router>,
    opened: tokio::sync::Mutex<Option<Opened>>,
}

impl Door {
    pub fn new(data: PathBuf, daemon_port: u16) -> Self {
        Self {
            data,
            daemon_port,
            router: OnceLock::new(),
            opened: tokio::sync::Mutex::default(),
        }
    }

    /// 문이 낼 라우터. 데몬이 사람 문과 같은 라우터를 만든 뒤 한 번 넣는다.
    pub fn set_router(&self, router: Router) {
        let _ = self.router.set(router);
    }

    /// 열려 있지 않으면 연다. 실패는 기억하지 않는다. 다음 부름이 다시 연다.
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
                eprintln!("loom: phone door stopped: {e}");
            }
        });
        *opened = Some(open.clone());
        Ok(open)
    }

    pub async fn current(&self) -> Option<Opened> {
        self.opened.lock().await.clone()
    }
}

/// 방화벽 규칙 이름. 다시 누르면 같은 규칙을 지우고 새로 넣는다.
const FIREWALL_RULE: &str = "loom-phone";

/// Windows 방화벽에 「개인 네트워크, 들어오는 TCP <port>, 이 실행 파일」 허용 규칙을 넣고, 이 실행 파일을 막는 규칙을 지운다.
/// Orca의 [휴대폰 연결 허용]과 같은 명령이다. 관리자 권한이 있어야 해서 UAC 확인 창을 띄운다(`Start-Process -Verb RunAs`).
/// 사람이 Windows의 첫 허용 창에서 [취소]를 누르면 Windows가 막는 규칙을 만든다. 그래서 막는 규칙도 지운다.
pub async fn allow_firewall(port: u16) -> std::io::Result<()> {
    if !cfg!(windows) {
        return Err(std::io::Error::other(
            "the firewall rule is only for Windows",
        ));
    }
    let exe = ps_quote(&std::env::current_exe()?.to_string_lossy());
    let rule = ps_quote(FIREWALL_RULE);
    let inner = format!(
        "$ErrorActionPreference = 'Stop'\n\
         Get-NetFirewallApplicationFilter -Program {exe} -ErrorAction SilentlyContinue | Get-NetFirewallRule | \
         Where-Object {{ $_.Direction -eq 'Inbound' -and $_.Action -eq 'Block' }} | Remove-NetFirewallRule\n\
         Get-NetFirewallRule -Name {rule} -ErrorAction SilentlyContinue | Remove-NetFirewallRule\n\
         New-NetFirewallRule -Name {rule} -DisplayName 'loom phone' -Direction Inbound -Action Allow -Profile Private \
         -Protocol TCP -LocalPort {port} -Program {exe} -EdgeTraversalPolicy Block | Out-Null"
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

/// PowerShell의 작은따옴표 글.
fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// 폰이 닿을 수 있는 이 기계의 IPv4 주소. 같은 네트워크(사설 주소)가 먼저, Tailscale(100.64.0.0/10)이 뒤다.
pub fn addresses() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut found: Vec<Ipv4Addr> = interfaces
        .iter()
        // WSL은 `lo`에 사설 주소(10.255.255.254, DNS)를 하나 더 둔다.
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

/// Tailscale이 기기에 주는 주소(CGNAT 범위 100.64.0.0/10).
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

/// 인증서와 키를 데이터 폴더에서 읽는다. 없거나 읽지 못하면 새로 만든다(그러면 짝지은 앱은 QR을 다시 찍는다).
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

/// Windows의 `%APPDATA%`는 사용자만 읽는다.
#[cfg(not(unix))]
fn write_secret(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn fingerprint(cert: &CertificateDer<'_>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(cert.as_ref()))
}

/// TLS 악수를 연결마다 따로 돌려, 느린 연결 하나가 다른 연결을 막지 않게 한다.
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
            // 받는 고리는 이 리스너보다 오래 산다. 닫히면 더 받을 연결이 없다.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_quotes() {
        assert_eq!(
            ps_quote(r"C:\Users\o'neil\loom.exe"),
            r"'C:\Users\o''neil\loom.exe'"
        );
    }

    #[test]
    fn tailnet_range() {
        assert!(tailnet(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(tailnet(Ipv4Addr::new(100, 127, 255, 1)));
        assert!(!tailnet(Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!tailnet(Ipv4Addr::new(192, 168, 0, 2)));
    }

    #[test]
    fn virtual_adapters_are_dropped() {
        for name in [
            "docker0",
            "vEthernet (WSL)",
            "VMware Network Adapter VMnet8",
            "br-1a2b",
            "veth12",
        ] {
            assert!(virtual_adapter(name), "{name}");
        }
        for name in ["Wi-Fi", "eth0", "wlan0", "en0", "Tailscale", "이더넷"] {
            assert!(!virtual_adapter(name), "{name}");
        }
    }

    #[test]
    fn addresses_are_private_or_tailnet() {
        let found = addresses();
        println!("phone door addresses: {found:?}");
        assert!(found.iter().all(|ip| ip.is_private() || tailnet(*ip)));
        assert!(!found.contains(&Ipv4Addr::new(10, 255, 255, 254)));
    }

    #[test]
    fn certificate_is_kept_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let (_, first) = server_config(dir.path()).unwrap();
        let (_, again) = server_config(dir.path()).unwrap();
        assert_eq!(first, again);
        assert_eq!(first.len(), 43);
    }
}
