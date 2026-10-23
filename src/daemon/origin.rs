//! 사람이 연 다른 웹 페이지가 데몬을 부르지 못하게 한다.
//!
//! 사람 문에는 토큰이 없다. 브라우저는 JSON 요청을 사전 요청으로 막지만, 두 길은 막지 않는다.
//! - 웹소켓: 출처와 관계없이 연결한다. 다른 페이지가 터미널 웹소켓에 글을 넣을 수 있다.
//! - DNS 재바인딩: 공격자 도메인이 127.0.0.1을 가리키게 바꾸면 같은 출처가 되어 모든 API를 부른다.
//!
//! 그래서 `Host`는 IP 주소나 `localhost`만 받고, `Origin`이 있으면 `http://<Host>`와 같아야 한다.
//! 노드의 `loom` 명령과 데스크톱 앱(데몬 주소를 그대로 연다)은 이 조건을 늘 지킨다.
//!
//! 폰은 Tailscale Serve로 닿는다(docs/design.md 「원격」). Serve는 HTTPS를 끝내고 루프백의 데몬에 넘기며,
//! `Tailscale-User-Login`·`X-Forwarded-*` 머리글을 붙인다. 다른 웹 페이지는 웹소켓과 교차 출처 요청에 이 머리글을 넣지 못한다.
//! 이 머리글 하나라도 있으면 Serve로 온 요청으로 본다. 주소가 `*.ts.net`이고 `Origin`이 없거나 `https://<그 주소>`여야 하고,
//! 짝지은 기기의 쿠키가 있어야 한다(`Devices`). 짝짓기 요청, 홈 화면 아이콘, 파일 통행증(`passes`)만 쿠키 없이 받는다.
//!
//! 폰 문(`lan`)으로 온 요청도 원격이다. 문이 `lan::Lan`을 넣는다. `Host`는 IP 주소이고 `Origin`은 없거나 `https://<Host>`여야 한다.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{lan, passes, phone};
use crate::devices::{DeviceId, Devices};

/// Tailscale Serve가 붙이는 요청자 로그인 머리글.
const TAILSCALE_LOGIN: &str = "tailscale-user-login";
/// Serve가 원래 주소를 적는 머리글. 없으면 `Host`를 본다.
const FORWARDED_HOST: &str = "x-forwarded-host";
const FORWARDED_FOR: &str = "x-forwarded-for";
/// Tailscale이 기기마다 주는 HTTPS 이름(MagicDNS)의 끝.
const TAILNET_SUFFIX: &str = ".ts.net";
/// 기기 토큰 쿠키.
pub const DEVICE_COOKIE: &str = "loom_device";
/// 짝짓기 전 폰이 받는 것: 짝짓기 요청, 「홈 화면에 추가」가 쿠키 없이 읽는 매니페스트와 아이콘(web/public), 짝짓기 쪽의 QR 판독기.
const UNPAIRED_PATHS: &[&str] = &["/manifest.webmanifest", "/icon.png", phone::JSQR_PATH];

/// 원격(Serve, 폰 문)으로 온 요청에 미들웨어가 넣는다. `device`는 짝지은 기기다(짝짓기 요청만 `None`).
/// 이 표시가 있는 요청은 PC 화면만 하는 일(짝짓기 코드, 기기 목록, Serve 켜기)을 하지 못한다.
#[derive(Clone, Copy)]
pub struct ViaRemote {
    pub device: Option<DeviceId>,
}

/// 노드 문. 노드는 늘 루프백으로 부른다.
pub async fn same_site(request: Request, next: Next) -> Response {
    if allowed(request.headers()) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// 사람 문. 이 기계의 화면과 Tailscale Serve·폰 문으로 온 짝지은 폰을 받는다. 라우터 전체(HTTP, 웹소켓, 문서, 파일)에 한 겹으로 씌운다.
pub async fn human_site(
    State(devices): State<Arc<Devices>>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let site = if request.extensions().get::<lan::Lan>().is_some() {
        lan_site(headers)
    } else if via_serve(headers) {
        serve_site(headers)
    } else {
        return same_site(request, next).await;
    };
    if !site {
        return StatusCode::FORBIDDEN.into_response();
    }
    let path = request.uri().path();
    let device = cookie(headers, DEVICE_COOKIE).and_then(|token| devices.check(token));
    let open = (request.method() == Method::POST && path == phone::PAIR_PATH)
        || (request.method() == Method::GET
            && (UNPAIRED_PATHS.contains(&path) || path.starts_with(passes::PASS_PREFIX)));
    if device.is_none() && !open {
        return phone::unpaired();
    }
    request.extensions_mut().insert(ViaRemote { device });
    next.run(request).await
}

/// 폰의 웹소켓(터미널, 로그인 창, 이벤트)을 PC에서 그 기기를 끊을 때 닫는다. 이 기계의 화면이면 `work`를 그대로 돌린다.
/// 문은 요청마다 기기를 보지만, 열린 웹소켓은 요청이 다시 오지 않는다.
pub async fn until_removed(
    devices: Arc<Devices>,
    via: Option<axum::Extension<ViaRemote>>,
    work: impl Future<Output = ()>,
) {
    match via.and_then(|v| v.device) {
        Some(id) => {
            tokio::select! {
                () = work => {}
                () = devices.removed(id) => {}
            }
        }
        None => work.await,
    }
}

fn via_serve(headers: &HeaderMap) -> bool {
    [TAILSCALE_LOGIN, FORWARDED_HOST, FORWARDED_FOR]
        .iter()
        .any(|name| headers.contains_key(*name))
        || headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .is_some_and(tailnet)
}

/// Serve로 온 요청의 주소와 출처.
fn serve_site(headers: &HeaderMap) -> bool {
    let text = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };
    let Some(real_host) = text(header::HOST.as_str()) else {
        return false;
    };
    // DNS 재바인딩: 남의 페이지가 127.0.0.1로 바꾼 이름에서 같은 출처로 부르면 머리글을 마음대로 넣고 `Origin` 없이 GET할 수 있다.
    // 그 페이지의 `Host`는 자기 이름이다. 그래서 진짜 `Host`도 DNS가 바꿀 수 없는 이름(Serve가 루프백에 보내는 IP)이나 tailnet 이름이어야 한다.
    if !fixed_host(real_host) && !tailnet(real_host) {
        return false;
    }
    let host = text(FORWARDED_HOST).unwrap_or(real_host);
    tailnet(host)
        && text(header::ORIGIN.as_str()).is_none_or(|origin| origin == format!("https://{host}"))
}

/// 폰 문으로 온 요청의 주소와 출처. 앱은 QR의 IP 주소로 연다. 이름이면 DNS 재바인딩일 수 있다.
fn lan_site(headers: &HeaderMap) -> bool {
    let text = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };
    let Some(host) = text(header::HOST.as_str()) else {
        return false;
    };
    host_ip(host)
        && text(header::ORIGIN.as_str()).is_none_or(|origin| origin == format!("https://{host}"))
}

/// `Cookie` 머리글에서 `name`의 값.
fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

/// Tailscale이 기기에 준 HTTPS 이름(`<기기>.<tailnet>.ts.net`)이다. 포트는 떼고 본다.
fn tailnet(host: &str) -> bool {
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    name.len() > TAILNET_SUFFIX.len() && name.to_ascii_lowercase().ends_with(TAILNET_SUFFIX)
}

fn allowed(headers: &HeaderMap) -> bool {
    let text = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let Some(host) = text(header::HOST) else {
        return false;
    };
    if !fixed_host(host) {
        return false;
    }
    text(header::ORIGIN).is_none_or(|origin| origin == format!("http://{host}"))
}

/// DNS가 바꿀 수 없는 이름이다. 포트는 떼고 본다.
fn fixed_host(host: &str) -> bool {
    host_name(host).eq_ignore_ascii_case("localhost") || host_ip(host)
}

fn host_ip(host: &str) -> bool {
    host_name(host).parse::<std::net::IpAddr>().is_ok()
}

/// `Host`에서 포트와 IPv6 괄호를 뗀 이름.
fn host_name(host: &str) -> &str {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !name.ends_with(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
            name
        }
        _ => host,
    };
    name.trim_start_matches('[').trim_end_matches(']')
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(host: Option<&'static str>, origin: Option<&'static str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = host {
            h.insert(header::HOST, HeaderValue::from_static(v));
        }
        if let Some(v) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn loopback_and_lan_addresses_pass() {
        for host in [
            "127.0.0.1:7420",
            "localhost:7420",
            "[::1]:7420",
            "192.168.0.5:7420",
            "127.0.0.1",
        ] {
            assert!(allowed(&headers(Some(host), None)), "{host}");
        }
        assert!(allowed(&headers(
            Some("127.0.0.1:7420"),
            Some("http://127.0.0.1:7420")
        )));
    }

    #[test]
    fn rebinding_and_other_origins_fail() {
        assert!(!allowed(&headers(Some("evil.example:7420"), None)));
        assert!(!allowed(&headers(None, None)));
        assert!(!allowed(&headers(
            Some("127.0.0.1:7420"),
            Some("https://evil.example")
        )));
        assert!(!allowed(&headers(
            Some("127.0.0.1:7420"),
            Some("http://localhost:7420")
        )));
        assert!(!allowed(&headers(Some("127.0.0.1:7420"), Some("null"))));
    }

    fn remote_headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(*name, HeaderValue::from_static(value));
        }
        h
    }

    const LOGIN: (&str, &str) = ("tailscale-user-login", "me@example.com");

    #[test]
    fn serve_headers_mark_remote() {
        assert!(via_serve(&remote_headers(&[LOGIN])));
        assert!(via_serve(&remote_headers(&[(
            "x-forwarded-for",
            "100.64.0.2"
        )])));
        assert!(via_serve(&remote_headers(&[(
            "x-forwarded-host",
            "pc.tail1234.ts.net"
        )])));
        assert!(via_serve(&remote_headers(&[(
            "host",
            "pc.tail1234.ts.net"
        )])));
        assert!(!via_serve(&remote_headers(&[("host", "127.0.0.1:7420")])));
    }

    #[test]
    fn serve_site_passes() {
        for pairs in [
            &[LOGIN, ("host", "pc.tail1234.ts.net")][..],
            &[
                LOGIN,
                ("host", "pc.tail1234.ts.net"),
                ("origin", "https://pc.tail1234.ts.net"),
            ],
            &[
                ("host", "127.0.0.1:7420"),
                ("x-forwarded-host", "pc.tail1234.ts.net"),
                ("origin", "https://pc.tail1234.ts.net"),
            ],
            &[
                ("host", "127.0.0.1:7420"),
                ("x-forwarded-host", "pc.tail1234.ts.net"),
            ],
            &[LOGIN, ("host", "PC.Tail1234.TS.NET")],
        ] {
            assert!(serve_site(&remote_headers(pairs)), "{pairs:?}");
        }
    }

    #[test]
    fn serve_site_fails() {
        for pairs in [
            &[LOGIN, ("host", "127.0.0.1:7420")][..],
            &[LOGIN, ("host", "evil.example")],
            &[LOGIN, ("host", ".ts.net")],
            // DNS 재바인딩: 남의 이름에서 같은 출처로 부른 GET(Origin 없음)은 X-Forwarded-Host를 꾸며도 막는다.
            &[
                LOGIN,
                ("host", "evil.com"),
                ("x-forwarded-host", "x.ts.net"),
            ],
            &[
                LOGIN,
                ("host", "127.0.0.1:7420"),
                ("x-forwarded-host", "evil.com"),
            ],
            &[
                LOGIN,
                ("host", "pc.tail1234.ts.net"),
                ("origin", "http://pc.tail1234.ts.net"),
            ],
            &[
                LOGIN,
                ("host", "pc.tail1234.ts.net"),
                ("origin", "https://evil.example"),
            ],
            &[LOGIN],
        ] {
            assert!(!serve_site(&remote_headers(pairs)), "{pairs:?}");
        }
    }

    #[test]
    fn cookie_finds_value() {
        let h = remote_headers(&[("cookie", "a=1; loom_device=tok-en_1; b=2")]);
        assert_eq!(cookie(&h, DEVICE_COOKIE), Some("tok-en_1"));
        assert_eq!(cookie(&h, "loom"), None);
        assert_eq!(cookie(&remote_headers(&[]), DEVICE_COOKIE), None);
    }

    mod gate {
        use super::*;
        use axum::Router;
        use axum::body::Body;
        use axum::extract::ws::WebSocketUpgrade;
        use axum::routing::{get, post};
        use tower::ServiceExt;

        /// 실제 라우터처럼 한 겹을 라우터 전체에 씌운다: 보통 API, 웹소켓, 질문 문서, 정적 파일(fallback), 짝짓기.
        fn router(devices: Arc<Devices>) -> Router {
            Router::new()
                .route("/api/repos", get(|| async { "repos" }))
                .route(
                    "/api/events",
                    get(|ws: WebSocketUpgrade| async move { ws.on_upgrade(|_| async {}) }),
                )
                .route("/api/docs/{id}/", get(|| async { "doc" }))
                .route(phone::PAIR_PATH, post(|| async { "pair" }))
                .fallback(|| async { "asset" })
                .layer(axum::middleware::from_fn_with_state(devices, human_site))
        }

        fn devices() -> (tempfile::TempDir, Arc<Devices>) {
            let dir = tempfile::tempdir().unwrap();
            let db = crate::db::open(&dir.path().join("t.db")).unwrap();
            (dir, Arc::new(Devices::new(db)))
        }

        fn remote(method: Method, path: &str, cookie: Option<&str>) -> Request {
            let mut b = Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:7420")
                .header("x-forwarded-host", "pc.tail1.ts.net")
                .header(TAILSCALE_LOGIN, "me@example.com");
            if let Some(c) = cookie {
                b = b.header("cookie", format!("{DEVICE_COOKIE}={c}"));
            }
            b.body(Body::empty()).unwrap()
        }

        fn upgrade(mut r: Request) -> Request {
            let h = r.headers_mut();
            h.insert("connection", HeaderValue::from_static("upgrade"));
            h.insert("upgrade", HeaderValue::from_static("websocket"));
            h.insert("sec-websocket-version", HeaderValue::from_static("13"));
            h.insert(
                "sec-websocket-key",
                HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
            );
            r
        }

        async fn status(devices: &Arc<Devices>, r: Request) -> StatusCode {
            router(devices.clone()).oneshot(r).await.unwrap().status()
        }

        /// 폰 문으로 온 요청: 문이 `lan::Lan`을 넣는다. 머리글은 앱의 WebView가 보내는 그대로다.
        fn lan(method: Method, path: &str, host: &str, origin: Option<&str>) -> Request {
            let mut b = Request::builder()
                .method(method)
                .uri(path)
                .header("host", host);
            if let Some(o) = origin {
                b = b.header("origin", o);
            }
            let mut r = b.body(Body::empty()).unwrap();
            r.extensions_mut().insert(lan::Lan);
            r
        }

        #[tokio::test]
        async fn lan_door_is_remote() {
            let (_dir, d) = devices();
            let host = "192.168.0.12:7430";
            // 쿠키 없는 폰 문 요청은 PC 화면과 달리 짝짓기 쪽을 받는다.
            assert_eq!(
                status(&d, lan(Method::GET, "/api/repos", host, None)).await,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                status(&d, upgrade(lan(Method::GET, "/api/events", host, None))).await,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                status(
                    &d,
                    lan(
                        Method::POST,
                        phone::PAIR_PATH,
                        host,
                        Some("https://192.168.0.12:7430")
                    )
                )
                .await,
                StatusCode::OK
            );
            // 이름 주소(DNS 재바인딩), 다른 출처, http 출처는 막는다.
            for (h, o) in [
                ("evil.example:7430", None),
                (host, Some("https://evil.example")),
                (host, Some("http://192.168.0.12:7430")),
            ] {
                assert_eq!(
                    status(&d, lan(Method::POST, phone::PAIR_PATH, h, o)).await,
                    StatusCode::FORBIDDEN,
                    "{h} {o:?}"
                );
            }
        }

        #[tokio::test]
        async fn every_remote_route_needs_a_paired_device() {
            let (_dir, d) = devices();
            for path in ["/api/repos", "/api/docs/R3/", "/", "/assets/index.js"] {
                assert_eq!(
                    status(&d, remote(Method::GET, path, None)).await,
                    StatusCode::UNAUTHORIZED,
                    "{path}"
                );
                assert_eq!(
                    status(&d, remote(Method::GET, path, Some("forged"))).await,
                    StatusCode::UNAUTHORIZED,
                    "{path}"
                );
            }
            assert_eq!(
                status(&d, upgrade(remote(Method::GET, "/api/events", None))).await,
                StatusCode::UNAUTHORIZED
            );
            // 짝짓기 요청, 홈 화면 아이콘, QR 판독기만 쿠키 없이 닿는다.
            assert_eq!(
                status(&d, remote(Method::POST, phone::PAIR_PATH, None)).await,
                StatusCode::OK
            );
            for path in ["/icon.png", phone::JSQR_PATH] {
                assert_eq!(
                    status(&d, remote(Method::GET, path, None)).await,
                    StatusCode::OK
                );
            }
            assert_eq!(
                status(&d, remote(Method::GET, phone::PAIR_PATH, None)).await,
                StatusCode::UNAUTHORIZED
            );
        }

        #[tokio::test]
        async fn paired_device_passes_until_removed() {
            let (_dir, d) = devices();
            let code = d.invite();
            let token = d.pair(&code, "a").unwrap().0;
            for path in ["/api/repos", "/api/docs/R3/", "/"] {
                assert_eq!(
                    status(&d, remote(Method::GET, path, Some(&token))).await,
                    StatusCode::OK,
                    "{path}"
                );
            }
            // oneshot에는 진짜 연결이 없어 업그레이드 단계에서 멈춘다. 문이 막지 않았다는 것(401이 아님)만 본다.
            assert_ne!(
                status(
                    &d,
                    upgrade(remote(Method::GET, "/api/events", Some(&token)))
                )
                .await,
                StatusCode::UNAUTHORIZED
            );
            let id = d.check(&token).unwrap();
            d.remove(id).unwrap();
            assert_eq!(
                status(&d, remote(Method::GET, "/api/repos", Some(&token))).await,
                StatusCode::UNAUTHORIZED
            );
        }

        #[tokio::test]
        async fn open_websocket_work_ends_when_device_is_removed() {
            let (_dir, d) = devices();
            let code = d.invite();
            let token = d.pair(&code, "a").unwrap().0;
            let id = d.check(&token).unwrap();
            let via = Some(axum::Extension(ViaRemote { device: Some(id) }));
            let work = tokio::spawn(until_removed(d.clone(), via, std::future::pending()));
            tokio::task::yield_now().await;
            assert!(!work.is_finished());
            d.remove(id).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), work)
                .await
                .unwrap()
                .unwrap();
            // 이 기계의 화면은 끊지 않는다.
            let local = tokio::spawn(until_removed(d.clone(), None, async {}));
            local.await.unwrap();
        }

        #[tokio::test]
        async fn local_requests_need_no_cookie_and_forged_origin_fails() {
            let (_dir, d) = devices();
            let local = Request::builder()
                .uri("/api/repos")
                .header("host", "127.0.0.1:7420")
                .body(Body::empty())
                .unwrap();
            assert_eq!(status(&d, local).await, StatusCode::OK);
            let mut bad = remote(Method::GET, "/api/repos", None);
            bad.headers_mut()
                .insert("origin", HeaderValue::from_static("https://evil.example"));
            assert_eq!(status(&d, bad).await, StatusCode::FORBIDDEN);
        }
    }
}
