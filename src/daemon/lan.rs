//! 폰 문: Tailscale 없이 같은 네트워크의 Android 앱이 닿는 HTTPS 문. 근거: docs/design.md 「원격」, 목업 docs/mockups/phone-direct.html.
//!
//! 사람 문(PC 화면)은 루프백에 그대로 둔다. 폰 문은 모든 IPv4 주소에서 듣고, 데몬이 처음 한 번 만든 자체 서명 인증서를 쓴다.
//! 브라우저는 이 인증서를 믿지 않는다. 앱은 QR에 담은 인증서 지문(SHA-256)과 같을 때만 연다(Orca가 QR의 PC 공개 키로 확인하는 것과 같다).
//! 문을 지나는 요청은 모두 원격이다(`origin::human_site`).
//!
//! 앱이 QR 없이 코드만 적으면 같은 Wi-Fi에 UDP로 묻는다(`FIND_PORT`). 열린 코드가 있으면 데몬이 폰 문 지문과 주소를
//! 코드로 확인한 답(`pake`)과 함께 돌려준다. 그래서 같은 Wi-Fi의 다른 기기는 PC인 척하지 못한다(사람 결정 Q426 B).

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

/// 폰 문 포트는 데몬 포트에서 이만큼 떨어진다. 앱이 저장한 주소가 데몬을 다시 띄워도 맞게 늘 같은 포트를 먼저 쓴다.
const PORT_OFFSET: u16 = 10;
/// TLS 악수를 기다리는 시간. 악수하지 않고 붙잡는 연결이 자원을 쥐지 않게 한다.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// 악수를 마친 연결을 axum에 넘기는 줄의 길이.
const BACKLOG: usize = 64;
/// 앱이 같은 Wi-Fi에서 PC를 찾는 UDP 포트. 앱은 데몬 포트를 모르므로 늘 같은 수다. 기본 폰 문 포트(7420+10)와 같은 수를 쓴다.
/// TCP와 UDP는 포트를 나누지 않는다. 다른 데몬(시험)이 이미 쓰고 있으면 이 데몬은 찾기에 답하지 않는다.
pub const FIND_PORT: u16 = 7430;
/// 찾기 물음 하나의 최대 크기. 물음은 `pA` 256바이트의 base64(약 350바이트)다.
const FIND_MAX: usize = 2048;
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

impl Opened {
    /// 앱이 폰 문에 붙는 값: `fp=<지문>&a=<IP:포트>,…`. QR의 `#` 뒤와 찾기 답이 같은 꼴을 쓴다.
    pub fn door(&self, addresses: &[Ipv4Addr]) -> String {
        let doors: Vec<String> = addresses
            .iter()
            .map(|ip| format!("{ip}:{}", self.port))
            .collect();
        format!("fp={}&a={}", self.fingerprint, doors.join(","))
    }
}

/// 폰 문은 처음 필요할 때 연다: 짝짓기 코드를 만들 때, 또는 짝지은 기기가 있는 채로 데몬이 시작할 때.
/// 폰을 쓰지 않는 사람의 데몬은 LAN에서 듣지 않는다(Windows 방화벽 창도 뜨지 않는다).
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
        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, FIND_PORT)).await {
            Ok(socket) => {
                tokio::spawn(answer_finds(socket, self.devices.clone(), open.clone()));
            }
            Err(e) => eprintln!("loom: phone find port {FIND_PORT}: {e}"),
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
    /// `pake` 물음 값(base64url).
    pa: String,
}

#[derive(Serialize, Deserialize)]
struct Found {
    /// `Opened::door`. 확인값이 이 글을 묶는다.
    door: String,
    pb: String,
    /// 확인값(base64url).
    c: String,
}

/// 찾기 물음에 답한다. 꼴이 틀린 물음은 세지 않고 버린다. 원소 확인과 답은 큰 수 계산이라 블로킹 스레드에서 한 번에 하나씩 한다.
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

/// 방화벽 규칙 이름. 다시 누르면 같은 규칙을 지우고 새로 넣는다.
const FIREWALL_RULE: &str = "loom-phone";
const FIREWALL_FIND_RULE: &str = "loom-phone-find";

/// Windows 방화벽에 「개인 네트워크, 들어오는 TCP <port>와 UDP `FIND_PORT`, 이 실행 파일」 허용 규칙을 넣고, 이 실행 파일을 막는 규칙을 지운다.
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

    #[tokio::test]
    async fn find_answers_only_with_the_code() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::open(&dir.path().join("t.db")).unwrap();
        let devices = Arc::new(Devices::new(db));
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let at = socket.local_addr().unwrap();
        let open = Opened {
            port: 7430,
            fingerprint: "FP".to_owned(),
        };
        tokio::spawn(answer_finds(socket, devices.clone(), open));
        let phone_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let ask = |phone: &pake::Phone| {
            serde_json::to_vec(&Find {
                pa: b64.encode(&phone.pa),
            })
            .unwrap()
        };
        let reply = async |request: &[u8]| {
            phone_socket.send_to(request, at).await.unwrap();
            let mut buf = vec![0u8; FIND_MAX];
            let got =
                tokio::time::timeout(Duration::from_millis(1500), phone_socket.recv(&mut buf))
                    .await;
            got.ok()
                .map(|len| serde_json::from_slice::<Found>(&buf[..len.unwrap()]).unwrap())
        };
        // 열린 코드가 없으면 답하지 않는다.
        let phone = pake::Phone::new("ABCD2345", 7);
        assert!(reply(&ask(&phone)).await.is_none());
        let code = devices.invite();
        // 꼴이 틀린 물음은 세지 않는다.
        assert!(reply(b"{\"pa\":\"AAAA\"}").await.is_none());
        assert!(reply(b"not json").await.is_none());
        let phone = pake::Phone::new(&code, 7);
        let found = reply(&ask(&phone)).await.unwrap();
        assert!(found.door.starts_with("fp=FP&a="));
        let (pb, c) = (
            b64.decode(&found.pb).unwrap(),
            b64.decode(&found.c).unwrap(),
        );
        assert!(phone.confirms(&pb, found.door.as_bytes(), &c));
        // 다른 PC가 지문을 바꿔 넘기면 확인값이 맞지 않는다.
        assert!(!phone.confirms(&pb, b"fp=MITM&a=10.0.0.9:7430", &c));
        let guess = pake::Phone::new("WRONGXX2", 7);
        assert!(!guess.confirms(&pb, found.door.as_bytes(), &c));
        assert!(devices.pair(&code, "Android · Chrome").is_ok());
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
