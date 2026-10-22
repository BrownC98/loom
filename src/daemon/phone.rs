//! 설정 「휴대폰」 탭과 폰의 짝짓기 라우트. 근거: docs/design.md 「원격」, 목업 docs/mockups/phone-pairing.html·phone-direct.html.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::lan;
use super::origin::{DEVICE_COOKIE, ViaRemote};
use super::{App, Problem, internal};
use crate::devices::{DeviceId, DeviceView, INVITE_TTL, PairRefused};
use crate::tailscale::{Served, Tailnet, Tailscale};

/// 폰이 코드를 보내는 곳. 문(`origin::human_site`)이 쿠키 없이 받는 단 하나의 API다.
pub const PAIR_PATH: &str = "/api/pair";
/// 짝짓기 쪽이 BarcodeDetector가 없을 때(iPhone Safari) 싣는 QR 판독기. 그 쪽은 앱 번들을 싣지 않는다.
pub const JSQR_PATH: &str = "/jsQR.js";
/// jsQR의 UMD 파일. 판은 web/package.json이 고정하고, Rust 빌드 전에 화면 빌드가 `npm ci`로 깐다.
const JSQR: &str = include_str!("../../web/node_modules/jsqr/dist/jsQR.js");
/// 쿠키 수명. 브라우저가 받는 최대(400일)다. 끊기는 PC의 [연결 끊기]로 한다.
const COOKIE_MAX_AGE_SECS: u64 = 400 * 24 * 60 * 60;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/phone", get(phone))
        .route("/api/phone/invite", post(invite))
        .route("/api/phone/serve", post(serve))
        .route("/api/phone/firewall", post(firewall))
        .route("/api/phone/devices/{id}", delete(remove))
        .route(PAIR_PATH, post(pair))
        .route(JSQR_PATH, get(jsqr))
}

async fn jsqr() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        JSQR,
    )
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhoneView {
    /// 폰 문이 QR에 담을 주소. 같은 네트워크(사설 주소)가 먼저, Tailscale이 뒤다.
    pub addresses: Vec<PhoneAddress>,
    /// 연 폰 문의 포트. [휴대폰 연결]을 누르기 전이면 없다.
    pub port: Option<u16>,
    /// [방화벽 허용]을 보일지. Windows 데몬만 그렇다.
    pub firewall: bool,
    /// 브라우저(iPhone)가 쓰는 Tailscale Serve 길의 상태.
    pub tailnet: Tailnet,
    pub devices: Vec<DeviceView>,
    /// 화면이 보인 코드를 아직 쓸 수 있는지. 폰이 쓰거나 시간이 지나면 `false`다.
    pub invite_open: bool,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhoneAddress {
    pub ip: String,
    /// Tailscale 주소(100.64.0.0/10)다. 폰도 Tailscale에 있으면 밖에서도 닿는다.
    pub tailnet: bool,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InviteView {
    /// QR에 담는 주소. 코드는 `#` 뒤에 있어 브라우저가 서버에 보내지 않는다.
    pub url: String,
    pub code: String,
    /// `url`의 QR 그림(SVG 글).
    pub qr_svg: String,
    #[ts(type = "number")]
    pub ttl_secs: u64,
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
}

/// PC 화면만 하는 일. Serve로 온 폰(짝지은 기기도)은 403이다.
fn local_only(via: Option<Extension<ViaRemote>>) -> Result<(), StatusCode> {
    match via {
        Some(_) => Err(StatusCode::FORBIDDEN),
        None => Ok(()),
    }
}

async fn phone_view(app: &App) -> Result<PhoneView, Problem> {
    let tailnet = match Tailscale::find() {
        Some(ts) => ts.status(app.port).await,
        None => Tailnet::Missing,
    };
    Ok(PhoneView {
        addresses: lan::addresses()
            .into_iter()
            .map(|ip| PhoneAddress {
                ip: ip.to_string(),
                tailnet: lan::tailnet(ip),
            })
            .collect(),
        port: app.door.current().await.map(|open| open.port),
        firewall: cfg!(windows),
        tailnet,
        devices: app.devices.list().map_err(internal)?,
        invite_open: app.devices.invite_open(),
    })
}

/// 이 탭 라우트의 실패. 상태 코드만 보내거나(PC 화면 아님, Tailscale 없음, 없는 기기) 원인 글을 보낸다.
enum Refused {
    Status(StatusCode),
    Problem(Problem),
}

impl From<StatusCode> for Refused {
    fn from(status: StatusCode) -> Self {
        Refused::Status(status)
    }
}

impl From<Problem> for Refused {
    fn from(problem: Problem) -> Self {
        Refused::Problem(problem)
    }
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        match self {
            Refused::Status(status) => status.into_response(),
            Refused::Problem(problem) => problem.into_response(),
        }
    }
}

async fn phone(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Result<Json<PhoneView>, Refused> {
    local_only(via)?;
    Ok(Json(phone_view(&app).await?))
}

async fn serve(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Result<Json<Served>, Refused> {
    local_only(via)?;
    let ts = Tailscale::find().ok_or(StatusCode::CONFLICT)?;
    Ok(Json(ts.serve(app.port).await.map_err(internal)?))
}

async fn invite(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Result<Json<InviteView>, Refused> {
    local_only(via)?;
    let open = app.door.open().await.map_err(internal)?;
    let serve = match Tailscale::find() {
        Some(ts) => match ts.status(app.port).await {
            Tailnet::Ready {
                host,
                serving: true,
            } => Some(host),
            _ => None,
        },
        None => None,
    };
    let addresses = lan::addresses();
    if addresses.is_empty() && serve.is_none() {
        return Err(StatusCode::CONFLICT.into());
    }
    let code = app.devices.invite();
    let url = invite_url(serve.as_deref(), &addresses, &open, &code);
    Ok(Json(InviteView {
        qr_svg: qr_svg(&url).map_err(internal)?,
        url,
        code: format!("{}-{}", &code[..4], &code[4..]),
        ttl_secs: INVITE_TTL.as_secs(),
    }))
}

async fn remove(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
    Path(id): Path<i64>,
) -> Result<Json<()>, Refused> {
    local_only(via)?;
    match app.devices.remove(DeviceId(id)).map_err(internal)? {
        true => Ok(Json(())),
        false => Err(StatusCode::NOT_FOUND.into()),
    }
}

/// Windows 방화벽에 폰 문 포트를 연다. 관리자 확인 창이 뜬다.
async fn firewall(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Result<Json<()>, Refused> {
    local_only(via)?;
    let open = app.door.open().await.map_err(internal)?;
    lan::allow_firewall(open.port).await.map_err(internal)?;
    Ok(Json(()))
}

/// QR 주소: `https://<호스트>/#pair=<코드>&fp=<인증서 지문>&a=<IP:포트>,…`.
/// 호스트는 Serve가 켜져 있으면 tailnet 이름(브라우저가 연다), 아니면 첫 주소다. 앱은 `fp`·`a`로 폰 문에 붙는다.
/// `#` 뒤는 브라우저가 서버에 보내지 않는다.
fn invite_url(
    serve: Option<&str>,
    addresses: &[std::net::Ipv4Addr],
    open: &lan::Opened,
    code: &str,
) -> String {
    let doors: Vec<String> = addresses
        .iter()
        .map(|ip| format!("{ip}:{}", open.port))
        .collect();
    let host = serve.map_or_else(|| doors[0].clone(), str::to_owned);
    let mut url = format!("https://{host}/#pair={code}");
    if !doors.is_empty() {
        url.push_str(&format!("&fp={}&a={}", open.fingerprint, doors.join(",")));
    }
    url
}

/// 원격(Serve, 폰 문)으로 온 요청만 받는다. 이 기계의 화면은 짝짓지 않는다(쿠키가 `localhost`에 남을 뿐이다).
async fn pair(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    if via.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    match app.devices.pair(&request.code, &label(ua)) {
        Ok(token) => (
            StatusCode::NO_CONTENT,
            [(header::SET_COOKIE, device_cookie(&token.0))],
        )
            .into_response(),
        Err(PairRefused::Wrong) => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// `Secure`: Serve와 폰 문이 HTTPS다. `HttpOnly`: 화면 스크립트가 읽지 못한다. `SameSite=Strict`: 다른 사이트의 요청에 붙지 않는다.
fn device_cookie(token: &str) -> String {
    format!(
        "{DEVICE_COOKIE}={token}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}; Secure; HttpOnly; SameSite=Strict"
    )
}

fn qr_svg(url: &str) -> Result<String, qrcode::types::QrError> {
    Ok(qrcode::QrCode::new(url.as_bytes())?
        .render::<qrcode::render::svg::Color<'_>>()
        .quiet_zone(true)
        .build())
}

/// 기기 목록에 보이는 이름: 「iPhone · Safari」. 브라우저 글(User-Agent)에서 만든다.
fn label(ua: &str) -> String {
    let device = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Windows", "Windows"),
        ("Macintosh", "Mac"),
        ("Linux", "Linux"),
    ]
    .into_iter()
    .find(|(key, _)| ua.contains(key))
    .map_or("Device", |(_, name)| name);
    // 순서가 뜻이다: Edge·Samsung·Chrome iOS(CriOS)의 글에도 「Safari」·「Chrome」이 들어 있다.
    let browser = [
        ("EdgA", "Edge"),
        ("EdgiOS", "Edge"),
        ("Edg/", "Edge"),
        ("SamsungBrowser", "Samsung Internet"),
        ("CriOS", "Chrome"),
        ("FxiOS", "Firefox"),
        ("Firefox", "Firefox"),
        ("Chrome", "Chrome"),
        ("Safari", "Safari"),
    ]
    .into_iter()
    .find(|(key, _)| ua.contains(key))
    .map(|(_, name)| name);
    match browser {
        Some(browser) => format!("{device} · {browser}"),
        None => device.to_owned(),
    }
}

/// 짝짓지 않은 폰이 받는 쪽(목업 phone-pairing.html ④, pair-qr-scan.html). 앱은 싣지 않는다: 노드와 질문을 보이지 않는다.
/// 주소의 `#pair=<코드>`(QR)를 읽어 바로 보낸다. 없으면 [QR 찍기](카메라)와 코드 칸을 보인다. 글은 폰 언어(한국어가 아니면 영어)로 고른다.
pub fn unpaired() -> Response {
    (StatusCode::UNAUTHORIZED, Html(UNPAIRED_PAGE)).into_response()
}

// 코드 칸: 네모 8칸 뒤에 보이지 않는 입력 칸 하나를 둔다(사람 결정 Q388 "ㅁㅁㅁㅁ-ㅁㅁㅁㅁ 꼴").
// 칸마다 입력을 두면 붙여 넣기·지우기·one-time-code 자동 채우기를 손으로 다시 만들어야 한다.
// maxlength를 두지 않는다: `ABCD-EFGH`(9자)를 붙여 넣으면 잘리기 전에 `-`를 빼야 한다.
// QR은 이 쪽과 같은 호스트의 `#pair=` 주소만 받는다. 다른 PC나 다른 사이트의 QR로 짝짓지 않는다.
const UNPAIRED_PAGE: &str = r#"<!doctype html><html lang="ko"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>loom</title>
<link rel="manifest" href="/manifest.webmanifest"><link rel="apple-touch-icon" href="/icon.png">
<style>:root{color-scheme:light dark}body{margin:0;padding:64px 20px 20px;font:15px/1.5 system-ui,sans-serif;background:light-dark(#f5f5f5,#1f1f1f);color:light-dark(#171717,#fafafa)}
section{display:grid;gap:12px;max-width:420px;margin:0 auto;padding:18px;border:1px solid light-dark(#e5e5e5,#454545);border-radius:12px;background:light-dark(#fff,#262626)}
.mark{width:32px;height:32px;border-radius:8px;background:light-dark(#171717,#e5e5e5);color:light-dark(#fafafa,#171717);display:grid;place-items:center;font:700 16px ui-monospace,monospace}
h1{margin:0;font-size:17px}p{margin:0;color:light-dark(#404040,#c4c4c4)}
button{min-height:44px;display:flex;align-items:center;justify-content:center;gap:6px;border:1px solid light-dark(#171717,#e5e5e5);border-radius:10px;background:light-dark(#171717,#e5e5e5);color:light-dark(#fafafa,#171717);font:500 15px system-ui,sans-serif}
button.sec{border-color:light-dark(#d4d4d4,#5c5c5c);background:light-dark(#fff,#262626);color:inherit}button svg{width:18px;height:18px}
.or{display:flex;align-items:center;gap:10px;font-size:12px;color:light-dark(#737373,#a3a3a3)}.or::before,.or::after{content:"";flex:1;border-top:1px solid light-dark(#e5e5e5,#454545)}
.pin{position:relative;display:flex;align-items:center;gap:6px}.pin span{flex:1;height:48px;border:1px solid light-dark(#d4d4d4,#5c5c5c);border-radius:8px;display:grid;place-items:center;font:600 20px ui-monospace,monospace}
.pin span.at{border:2px solid light-dark(#171717,#fafafa)}.pin i{font:600 20px ui-monospace,monospace;font-style:normal;color:light-dark(#737373,#a3a3a3)}
.pin input{position:absolute;inset:0;width:100%;opacity:0;border:0;font-size:16px;caret-color:transparent}
.cam{position:relative;aspect-ratio:1;border-radius:10px;overflow:hidden;background:#222}.cam video{width:100%;height:100%;object-fit:cover;display:block}
.cam .frame{position:absolute;inset:19%;border:3px solid rgba(255,255,255,.9);border-radius:12px;box-shadow:0 0 0 999px rgba(0,0,0,.35)}
.err{color:light-dark(#c0392b,#ef7b6e);font-size:13px}[hidden]{display:none}</style></head><body>
<section id="ask"><span class="mark">L</span><h1 data-t="title"></h1><p data-t="body"></p><span id="camErr" class="err" hidden data-t="noCamera"></span>
<button id="scan" type="button" hidden><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3"/><rect x="9" y="9" width="6" height="6"/></svg><span data-t="scan"></span></button>
<div id="or" class="or" hidden data-t="or"></div>
<form id="form"><div style="display:grid;gap:12px"><label class="pin"><input id="code" autocomplete="one-time-code" autocapitalize="characters" autocorrect="off" spellcheck="false" inputmode="text"><span></span><span></span><span></span><span></span><i>-</i><span></span><span></span><span></span><span></span></label>
<span id="err" class="err" hidden data-t="wrong"></span><button id="connect" class="sec" data-t="connect"></button></div></form></section>
<section id="scanning" hidden><h1 data-t="scanTitle"></h1><div class="cam"><video id="video" playsinline muted></video><div class="frame"></div></div><p id="scanBody" data-t="scanBody"></p><span id="otherQr" class="err" hidden data-t="otherQr"></span><button id="cancel" class="sec" type="button" data-t="cancel"></button></section>
<section id="done" hidden><span class="mark">L</span><h1 data-t="doneTitle"></h1><p data-t="doneBody"></p><button id="start" data-t="start"></button></section>
<script>
const T={ko:{title:'이 기기는 연결되지 않았습니다',body:'PC의 loom 설정 › 휴대폰 › [휴대폰 연결]에 나온 QR을 찍거나 코드를 적습니다.',code:'코드 8자',scan:'QR 찍기',or:'또는 코드 적기',connect:'연결',wrong:'코드가 맞지 않거나 시간이 지났습니다. PC에서 새 코드를 만듭니다.',noCamera:'카메라를 쓸 수 없습니다. 코드를 적습니다.',scanTitle:'PC 화면의 QR을 찍습니다',scanBody:'QR을 네모 안에 맞춥니다. 읽으면 바로 연결합니다.',otherQr:'이 QR은 이 loom의 연결 QR이 아닙니다. PC의 설정 › 휴대폰에 나온 QR을 찍습니다.',cancel:'취소',doneTitle:'loom에 연결했습니다',doneBody:'이 기기를 PC의 loom에 연결했습니다. 홈 화면에 추가하면 앱처럼 씁니다.',start:'시작'},
en:{title:'This device is not connected',body:'On the PC, open loom Settings › Phone › [Connect phone]. Scan the QR code shown there or enter the code.',code:'8-character code',scan:'Scan QR code',or:'or enter the code',connect:'Connect',wrong:'The code is wrong or expired. Make a new code on the PC.',noCamera:'The camera is not available. Enter the code.',scanTitle:'Scan the QR code on the PC',scanBody:'Put the QR code in the square. loom connects when it reads the code.',otherQr:'This QR code is not a connect code for this loom. Scan the QR code in Settings › Phone on the PC.',cancel:'Cancel',doneTitle:'Connected to loom',doneBody:'This device is connected to loom on the PC. Add it to the home screen to use it like an app.',start:'Start'}};
const lang=navigator.language.toLowerCase().startsWith('ko')?'ko':'en';const t=T[lang];document.documentElement.lang=lang;
for(const el of document.querySelectorAll('[data-t]'))el.textContent=t[el.dataset.t];
const $=id=>document.getElementById(id);const input=$('code');const boxes=[...document.querySelectorAll('.pin span')];input.setAttribute('aria-label',t.code);
function show(id){for(const s of ['ask','scanning','done'])$(s).hidden=s!==id;}
let busy=false;
async function pair(code){if(busy)return;busy=true;$('err').hidden=true;const r=await fetch('/api/pair',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({code})}).catch(()=>null);busy=false;
if(r&&r.ok){show('done');}else{show('ask');$('err').hidden=false;}}
function draw(){const v=input.value,on=document.activeElement===input;boxes.forEach((b,i)=>{b.textContent=v[i]||'';b.classList.toggle('at',on&&i===Math.min(v.length,7));});}
input.addEventListener('input',()=>{input.value=input.value.toUpperCase().replace(/[^A-Z0-9]/g,'').slice(0,8);input.setSelectionRange(8,8);$('err').hidden=true;draw();if(input.value.length===8)pair(input.value);});
for(const e of ['focus','blur','click','keyup'])input.addEventListener(e,()=>{if(e!=='blur')input.setSelectionRange(8,8);draw();});
$('form').addEventListener('submit',e=>{e.preventDefault();pair(input.value);});
$('start').addEventListener('click',()=>location.replace('/'));
function codeFrom(text){try{const u=new URL(text);const m=u.hash.match(/^#pair=([A-Za-z0-9-]+)(&|$)/);return u.protocol===location.protocol&&u.host===location.host&&m?m[1]:null;}catch{return null;}}
let stream=null;
function stop(){if(stream)for(const tr of stream.getTracks())tr.stop();stream=null;$('video').srcObject=null;}
async function reader(video){if('BarcodeDetector' in window&&(await BarcodeDetector.getSupportedFormats()).includes('qr_code')){const d=new BarcodeDetector({formats:['qr_code']});return async()=>(await d.detect(video))[0]?.rawValue;}
if(!window.jsQR)await new Promise((ok,no)=>{const s=document.createElement('script');s.src='/jsQR.js';s.onload=ok;s.onerror=no;document.head.append(s);});
const c=document.createElement('canvas'),x=c.getContext('2d',{willReadFrequently:true});return async()=>{const w=video.videoWidth,h=video.videoHeight;if(!w)return null;c.width=w;c.height=h;x.drawImage(video,0,0,w,h);return jsQR(x.getImageData(0,0,w,h).data,w,h,{inversionAttempts:'dontInvert'})?.data;};}
async function scan(){$('camErr').hidden=true;$('err').hidden=true;$('otherQr').hidden=true;const video=$('video');let read;
try{stream=await navigator.mediaDevices.getUserMedia({video:{facingMode:'environment'},audio:false});video.srcObject=stream;await video.play();read=await reader(video);}catch{stop();show('ask');$('camErr').hidden=false;return;}
show('scanning');while(stream){const text=await read().catch(()=>null);if(!stream)return;if(text){const code=codeFrom(text);if(code){stop();pair(code);return;}$('otherQr').hidden=false;$('scanBody').hidden=true;}await new Promise(r=>setTimeout(r,200));}}
$('scan').addEventListener('click',scan);$('cancel').addEventListener('click',()=>{stop();show('ask');});
if(navigator.mediaDevices?.getUserMedia){$('scan').hidden=false;$('or').hidden=false;}
draw();
const m=location.hash.match(/pair=([A-Za-z0-9-]+)/);if(m){history.replaceState(null,'',location.pathname);pair(m[1]);}
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_is_secure_httponly_strict() {
        let c = device_cookie("tok");
        assert!(c.starts_with("loom_device=tok; "));
        for flag in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
            assert!(c.split("; ").any(|part| part == flag), "{flag}: {c}");
        }
        assert!(!c.contains("Domain="));
    }

    #[test]
    fn labels_from_user_agents() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        assert_eq!(label(iphone), "iPhone · Safari");
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36";
        assert_eq!(label(android), "Android · Chrome");
        let crios = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/130.0 Mobile/15E148 Safari/604.1";
        assert_eq!(label(crios), "iPhone · Chrome");
        assert_eq!(label(""), "Device");
    }

    #[test]
    fn qr_encodes_url() {
        let svg = qr_svg("https://pc.tail1.ts.net/#pair=ABCDEFGH").unwrap();
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn invite_url_carries_doors_and_pin() {
        let open = lan::Opened {
            port: 7430,
            fingerprint: "FP".to_owned(),
        };
        let lan = [
            std::net::Ipv4Addr::new(192, 168, 0, 12),
            std::net::Ipv4Addr::new(100, 101, 12, 7),
        ];
        assert_eq!(
            invite_url(None, &lan, &open, "ABCDEFGH"),
            "https://192.168.0.12:7430/#pair=ABCDEFGH&fp=FP&a=192.168.0.12:7430,100.101.12.7:7430"
        );
        assert_eq!(
            invite_url(Some("pc.tail1.ts.net"), &lan[..1], &open, "ABCDEFGH"),
            "https://pc.tail1.ts.net/#pair=ABCDEFGH&fp=FP&a=192.168.0.12:7430"
        );
        assert_eq!(
            invite_url(Some("pc.tail1.ts.net"), &[], &open, "ABCDEFGH"),
            "https://pc.tail1.ts.net/#pair=ABCDEFGH"
        );
    }

    #[test]
    fn local_only_refuses_serve_requests() {
        assert!(local_only(None).is_ok());
        assert_eq!(
            local_only(Some(Extension(ViaRemote { device: None }))),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            local_only(Some(Extension(ViaRemote {
                device: Some(DeviceId(1))
            }))),
            Err(StatusCode::FORBIDDEN)
        );
    }
}
