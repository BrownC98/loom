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
use crate::devices::{DeviceId, DeviceView, PairRefused};
use crate::tailscale::{Served, Tailnet, Tailscale};

pub const PAIR_PATH: &str = "/api/pair";
pub const JSQR_PATH: &str = "/jsQR.js";
const JSQR: &str = include_str!("../../web/node_modules/jsqr/dist/jsQR.js");
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
    /// 화면이 보인 코드를 아직 쓸 수 있는지. 폰이 쓰거나 다섯 번 틀리거나 시간이 지나면 `false`다. 앞의 둘은 `ViewChanged`로 알린다.
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
    /// 코드를 쓸 수 있는 마지막 때(Unix 밀리초). 화면은 남은 시간을 보이기만 하고, 쓸 수 있는지는 데몬이 쓸 때 본다.
    #[ts(type = "number")]
    pub until: i64,
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
}

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
    let serve = serve_host(app.port).await;
    let addresses = lan::addresses();
    if addresses.is_empty() && serve.is_none() {
        return Err(StatusCode::CONFLICT.into());
    }
    let invitation = app.devices.invite();
    let code = invitation.code;
    let url = invite_url(serve.as_deref(), &addresses, &open, &code);
    Ok(Json(InviteView {
        qr_svg: qr_svg(&url).map_err(internal)?,
        url,
        code: format!("{}-{}", &code[..4], &code[4..]),
        until: invitation.until_ms,
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

async fn firewall(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Result<Json<()>, Refused> {
    local_only(via)?;
    let open = app.door.open().await.map_err(internal)?;
    lan::allow_firewall(open.port).await.map_err(internal)?;
    Ok(Json(()))
}

pub async fn serve_host(port: u16) -> Option<String> {
    match Tailscale::find()?.status(port).await {
        Tailnet::Ready {
            host,
            serving: true,
        } => Some(host),
        _ => None,
    }
}

fn invite_url(
    serve: Option<&str>,
    addresses: &[std::net::Ipv4Addr],
    open: &lan::Opened,
    code: &str,
) -> String {
    let host = serve.map_or_else(|| format!("{}:{}", addresses[0], open.port), str::to_owned);
    let mut url = format!("https://{host}/#pair={code}");
    if !addresses.is_empty() {
        url.push('&');
        url.push_str(&open.door(addresses));
    }
    url
}

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

pub fn unpaired() -> Response {
    (StatusCode::UNAUTHORIZED, Html(UNPAIRED_PAGE)).into_response()
}

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
