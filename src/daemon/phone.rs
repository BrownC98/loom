//! 설정 「휴대폰」 탭과 폰의 짝짓기 라우트. 근거: docs/design.md 「원격」, 목업 docs/mockups/phone-pairing.html.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::origin::{DEVICE_COOKIE, ViaServe};
use super::{App, Problem, internal};
use crate::devices::{DeviceId, DeviceView, INVITE_TTL, PairRefused};
use crate::tailscale::{Tailnet, Tailscale};

/// 폰이 코드를 보내는 곳. 문(`origin::human_site`)이 쿠키 없이 받는 단 하나의 API다.
pub const PAIR_PATH: &str = "/api/pair";
/// 쿠키 수명. 브라우저가 받는 최대(400일)다. 끊기는 PC의 [연결 끊기]로 한다.
const COOKIE_MAX_AGE_SECS: u64 = 400 * 24 * 60 * 60;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/phone", get(phone))
        .route("/api/phone/invite", post(invite))
        .route("/api/phone/serve", post(serve))
        .route("/api/phone/devices/{id}", delete(remove))
        .route(PAIR_PATH, post(pair))
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhoneView {
    pub tailnet: Tailnet,
    pub devices: Vec<DeviceView>,
    /// 화면이 보인 코드를 아직 쓸 수 있는지. 폰이 쓰거나 시간이 지나면 `false`다.
    pub invite_open: bool,
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
fn local_only(via: Option<Extension<ViaServe>>) -> Result<(), StatusCode> {
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
    via: Option<Extension<ViaServe>>,
) -> Result<Json<PhoneView>, Refused> {
    local_only(via)?;
    Ok(Json(phone_view(&app).await?))
}

async fn serve(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaServe>>,
) -> Result<Json<PhoneView>, Refused> {
    local_only(via)?;
    let ts = Tailscale::find().ok_or(StatusCode::CONFLICT)?;
    ts.serve(app.port).await.map_err(internal)?;
    Ok(Json(phone_view(&app).await?))
}

async fn invite(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaServe>>,
) -> Result<Json<InviteView>, Refused> {
    local_only(via)?;
    let ts = Tailscale::find().ok_or(StatusCode::CONFLICT)?;
    let Tailnet::Ready { host, .. } = ts.status(app.port).await else {
        return Err(StatusCode::CONFLICT.into());
    };
    let code = app.devices.invite();
    let url = format!("https://{host}/#pair={code}");
    Ok(Json(InviteView {
        qr_svg: qr_svg(&url).map_err(internal)?,
        url,
        code: format!("{}-{}", &code[..4], &code[4..]),
        ttl_secs: INVITE_TTL.as_secs(),
    }))
}

async fn remove(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaServe>>,
    Path(id): Path<i64>,
) -> Result<Json<()>, Refused> {
    local_only(via)?;
    match app.devices.remove(DeviceId(id)).map_err(internal)? {
        true => Ok(Json(())),
        false => Err(StatusCode::NOT_FOUND.into()),
    }
}

/// Serve로 온 요청만 받는다. 이 기계의 화면은 짝짓지 않는다(쿠키가 `localhost`에 남을 뿐이다).
async fn pair(
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaServe>>,
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

/// `Secure`: Serve가 HTTPS를 끝낸다. `HttpOnly`: 화면 스크립트가 읽지 못한다. `SameSite=Strict`: 다른 사이트의 요청에 붙지 않는다.
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

/// 짝짓지 않은 폰이 받는 쪽(목업 phone-pairing.html ④). 앱은 싣지 않는다: 노드와 질문을 보이지 않는다.
/// 주소의 `#pair=<코드>`(QR)를 읽어 바로 보내고, 없으면 코드 칸을 보인다. 글은 폰 언어(한국어가 아니면 영어)로 고른다.
pub fn unpaired() -> Response {
    (StatusCode::UNAUTHORIZED, Html(UNPAIRED_PAGE)).into_response()
}

const UNPAIRED_PAGE: &str = r#"<!doctype html><html lang="ko"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>loom</title>
<link rel="manifest" href="/manifest.webmanifest"><link rel="apple-touch-icon" href="/icon.png">
<style>:root{color-scheme:light dark}body{margin:0;padding:120px 20px 20px;font:15px/1.5 system-ui,sans-serif;background:light-dark(#f5f5f5,#1f1f1f);color:light-dark(#171717,#fafafa)}
section{display:grid;gap:12px;max-width:420px;margin:0 auto;padding:18px;border:1px solid light-dark(#e5e5e5,#454545);border-radius:12px;background:light-dark(#fff,#262626)}
.mark{width:32px;height:32px;border-radius:8px;background:light-dark(#171717,#e5e5e5);color:light-dark(#fafafa,#171717);display:grid;place-items:center;font:700 16px ui-monospace,monospace}
h1{margin:0;font-size:17px}p{margin:0;color:light-dark(#404040,#c4c4c4)}
input{min-height:48px;padding:0 12px;border:1px solid light-dark(#d4d4d4,#5c5c5c);border-radius:10px;background:inherit;color:inherit;font:600 20px ui-monospace,monospace;letter-spacing:2px;text-transform:uppercase}
input::placeholder{font:15px system-ui,sans-serif;letter-spacing:0;text-transform:none}
button{min-height:44px;border:0;border-radius:10px;background:light-dark(#171717,#e5e5e5);color:light-dark(#fafafa,#171717);font:500 15px system-ui,sans-serif}
.err{color:light-dark(#c0392b,#ef7b6e);font-size:13px}[hidden]{display:none}</style></head><body>
<section id="ask"><span class="mark">L</span><h1 data-t="title"></h1><p data-t="body"></p>
<form id="form"><div style="display:grid;gap:12px"><input id="code" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" maxlength="9" required><span id="err" class="err" hidden data-t="wrong"></span><button data-t="connect"></button></div></form></section>
<section id="done" hidden><span class="mark">L</span><h1 data-t="doneTitle"></h1><p data-t="doneBody"></p><button id="start" data-t="start"></button></section>
<script>
const T={ko:{title:'이 기기는 연결되지 않았습니다',body:'PC의 loom 설정 › 휴대폰 › [휴대폰 연결]에 나온 코드를 적습니다.',code:'코드 8자',connect:'연결',wrong:'코드가 맞지 않거나 시간이 지났습니다. PC에서 새 코드를 만듭니다.',doneTitle:'loom에 연결했습니다',doneBody:'이 기기를 PC의 loom에 연결했습니다. 홈 화면에 추가하면 앱처럼 씁니다.',start:'시작'},
en:{title:'This device is not connected',body:'On the PC, open loom Settings › Phone › [Connect phone] and enter the code shown there.',code:'8-character code',connect:'Connect',wrong:'The code is wrong or expired. Make a new code on the PC.',doneTitle:'Connected to loom',doneBody:'This device is connected to loom on the PC. Add it to the home screen to use it like an app.',start:'Start'}};
const lang=navigator.language.toLowerCase().startsWith('ko')?'ko':'en';const t=T[lang];document.documentElement.lang=lang;
for(const el of document.querySelectorAll('[data-t]'))el.textContent=t[el.dataset.t];
const input=document.getElementById('code');input.placeholder=t.code;
async function pair(code){const r=await fetch('/api/pair',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({code})}).catch(()=>null);
if(r&&r.ok){document.getElementById('ask').hidden=true;document.getElementById('done').hidden=false;}else{document.getElementById('err').hidden=false;}}
document.getElementById('form').addEventListener('submit',e=>{e.preventDefault();pair(input.value);});
document.getElementById('start').addEventListener('click',()=>location.replace('/'));
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
    fn local_only_refuses_serve_requests() {
        assert!(local_only(None).is_ok());
        assert_eq!(
            local_only(Some(Extension(ViaServe { device: None }))),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            local_only(Some(Extension(ViaServe {
                device: Some(DeviceId(1))
            }))),
            Err(StatusCode::FORBIDDEN)
        );
    }
}
