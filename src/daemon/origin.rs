use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{lan, passes, phone};
use crate::devices::{DeviceId, Devices};

const TAILSCALE_LOGIN: &str = "tailscale-user-login";
const FORWARDED_HOST: &str = "x-forwarded-host";
const FORWARDED_FOR: &str = "x-forwarded-for";
const TAILNET_SUFFIX: &str = ".ts.net";
pub const DEVICE_COOKIE: &str = "loom_device";
const UNPAIRED_PATHS: &[&str] = &["/manifest.webmanifest", "/icon.png", phone::JSQR_PATH];

#[derive(Clone, Copy)]
pub struct ViaRemote {
    pub device: Option<DeviceId>,
}

pub async fn same_site(request: Request, next: Next) -> Response {
    if allowed(request.headers()) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

pub async fn from_page(request: Request, next: Next) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

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

fn serve_site(headers: &HeaderMap) -> bool {
    let text = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };
    let Some(real_host) = text(header::HOST.as_str()) else {
        return false;
    };
    if !fixed_host(real_host) && !tailnet(real_host) {
        return false;
    }
    let host = text(FORWARDED_HOST).unwrap_or(real_host);
    tailnet(host)
        && text(header::ORIGIN.as_str()).is_none_or(|origin| origin == format!("https://{host}"))
}

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

fn fixed_host(host: &str) -> bool {
    host_name(host).eq_ignore_ascii_case("localhost") || host_ip(host)
}

fn host_ip(host: &str) -> bool {
    host_name(host).parse::<std::net::IpAddr>().is_ok()
}

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

        #[tokio::test]
        async fn opener_routes_need_the_page_origin() {
            let (_dir, devices) = devices();
            let app = Router::new()
                .route(
                    "/api/files/open",
                    post(|| async { "opened" }).layer(axum::middleware::from_fn(from_page)),
                )
                .layer(axum::middleware::from_fn_with_state(devices, human_site));
            let send = |origin: Option<&'static str>| {
                let mut b = Request::builder()
                    .method(Method::POST)
                    .uri("/api/files/open")
                    .header("host", "127.0.0.1:7420");
                if let Some(o) = origin {
                    b = b.header("origin", o);
                }
                app.clone().oneshot(b.body(Body::empty()).unwrap())
            };
            assert_eq!(send(None).await.unwrap().status(), StatusCode::FORBIDDEN);
            assert_eq!(
                send(Some("http://127.0.0.1:7420")).await.unwrap().status(),
                StatusCode::OK
            );
            assert_eq!(
                send(Some("https://evil.example")).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
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
