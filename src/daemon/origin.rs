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
//! 요청한 Tailscale 사용자를 `Tailscale-User-Login`에 적는다. 밖에서 온 같은 머리글은 Serve가 지운다.
//! 다른 웹 페이지는 웹소켓과 교차 출처 요청에 이 머리글을 넣지 못한다. 그래서 이 머리글이 있는 요청은
//! 로그인이 설정 「원격 허용 로그인」과 같고, 주소가 `*.ts.net`이고, `Origin`이 있으면 `https://<그 주소>`일 때만 받는다.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};

use super::App;

/// Tailscale Serve가 붙이는 요청자 로그인 머리글.
const TAILSCALE_LOGIN: &str = "tailscale-user-login";
/// Serve가 원래 주소를 적는 머리글. 없으면 `Host`를 본다.
const FORWARDED_HOST: &str = "x-forwarded-host";
/// Tailscale이 기기마다 주는 HTTPS 이름(MagicDNS)의 끝.
const TAILNET_SUFFIX: &str = ".ts.net";

/// 노드 문. 노드는 늘 루프백으로 부른다.
pub async fn same_site(request: Request, next: Next) -> Response {
    if allowed(request.headers()) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// 사람 문. 이 기계의 화면과 Tailscale Serve로 온 폰을 받는다.
pub async fn human_site(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let Some(login) = headers.get(TAILSCALE_LOGIN) else {
        return same_site(request, next).await;
    };
    let allowed_login = app.settings.get().ok().and_then(|s| s.remote_login);
    match remote(headers, login, allowed_login.as_deref()) {
        Remote::Allowed => next.run(request).await,
        Remote::LoginRefused(login) => refused(&login),
        Remote::OriginRefused => StatusCode::FORBIDDEN.into_response(),
    }
}

enum Remote {
    Allowed,
    /// 머리글의 로그인이 설정과 다르거나 설정이 비었다. 폰이 고칠 곳을 알게 그 로그인을 보인다.
    LoginRefused(String),
    OriginRefused,
}

fn remote(headers: &HeaderMap, login: &HeaderValue, allowed_login: Option<&str>) -> Remote {
    let login = login.to_str().unwrap_or_default().trim();
    if allowed_login
        .map(str::trim)
        .is_none_or(|allowed| allowed.is_empty() || !allowed.eq_ignore_ascii_case(login))
    {
        return Remote::LoginRefused(login.to_owned());
    }
    let text = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };
    let Some(real_host) = text(header::HOST.as_str()) else {
        return Remote::OriginRefused;
    };
    // DNS 재바인딩: 남의 페이지가 127.0.0.1로 바꾼 이름에서 같은 출처로 부르면 머리글을 마음대로 넣고 `Origin` 없이 GET할 수 있다.
    // 그 페이지의 `Host`는 자기 이름이다. 그래서 진짜 `Host`도 DNS가 바꿀 수 없는 이름(Serve가 루프백에 보내는 IP)이나 tailnet 이름이어야 한다.
    if !fixed_host(real_host) && !tailnet(real_host) {
        return Remote::OriginRefused;
    }
    let host = text(FORWARDED_HOST).unwrap_or(real_host);
    if tailnet(host)
        && text(header::ORIGIN.as_str()).is_none_or(|origin| origin == format!("https://{host}"))
    {
        Remote::Allowed
    } else {
        Remote::OriginRefused
    }
}

/// Tailscale이 기기에 준 HTTPS 이름(`<기기>.<tailnet>.ts.net`)이다. 포트는 떼고 본다.
fn tailnet(host: &str) -> bool {
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    name.len() > TAILNET_SUFFIX.len() && name.to_ascii_lowercase().ends_with(TAILNET_SUFFIX)
}

/// 폰 브라우저에 보이는 거절 쪽(목업 mobile.html ⑥의 카드). 앱은 싣지 않는다: 거절한 로그인에 노드 목록을 보이지 않는다.
/// 화면을 읽기 전이라 화면 언어를 모른다. 두 언어로 쓴다.
fn refused(login: &str) -> Response {
    let login = login
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let page = format!(
        r#"<!doctype html><html lang="ko"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>loom</title>
<style>:root{{color-scheme:light dark}}body{{margin:0;padding:16px;font:15px/1.5 system-ui,sans-serif;background:light-dark(#f5f5f5,#1f1f1f);color:light-dark(#171717,#fafafa)}}
section{{display:grid;gap:8px;max-width:480px;margin:0 auto 12px;padding:14px 16px;border:1px solid light-dark(#d4d4d4,#525252);border-radius:12px;background:light-dark(#fff,#262626)}}
h1{{margin:0;font-size:16px}}p{{margin:0;color:light-dark(#525252,#c4c4c4)}}code{{font:13px ui-monospace,monospace;overflow-wrap:anywhere}}</style></head><body>
<section><h1>이 로그인은 허용되지 않았습니다</h1><p>PC의 loom 설정 › 일반에 이 Tailscale 로그인을 넣으십시오.</p><code>{login}</code></section>
<section lang="en"><h1>This login is not allowed</h1><p>On the PC, open loom Settings › General and enter this Tailscale login.</p><code>{login}</code></section>
</body></html>"#
    );
    (StatusCode::FORBIDDEN, Html(page)).into_response()
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
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !name.ends_with(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
            name
        }
        _ => host,
    };
    let name = name.trim_start_matches('[').trim_end_matches(']');
    name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::IpAddr>().is_ok()
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

    fn verdict(
        pairs: &[(&'static str, &'static str)],
        allowed_login: Option<&str>,
    ) -> &'static str {
        let h = remote_headers(pairs);
        match remote(&h, h.get(TAILSCALE_LOGIN).unwrap(), allowed_login) {
            Remote::Allowed => "allowed",
            Remote::LoginRefused(_) => "login",
            Remote::OriginRefused => "origin",
        }
    }

    const LOGIN: (&str, &str) = ("tailscale-user-login", "me@example.com");

    #[test]
    fn tailscale_owner_passes() {
        let me = Some("me@example.com");
        // Serve가 Host를 그대로 넘기는 경우와 X-Forwarded-Host에 적는 경우.
        assert_eq!(
            verdict(&[LOGIN, ("host", "pc.tail1234.ts.net")], me),
            "allowed"
        );
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "pc.tail1234.ts.net"),
                    ("origin", "https://pc.tail1234.ts.net")
                ],
                me
            ),
            "allowed"
        );
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "127.0.0.1:7420"),
                    ("x-forwarded-host", "pc.tail1234.ts.net"),
                    ("origin", "https://pc.tail1234.ts.net")
                ],
                me
            ),
            "allowed"
        );
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "127.0.0.1:7420"),
                    ("x-forwarded-host", "pc.tail1234.ts.net")
                ],
                me
            ),
            "allowed"
        );
        assert_eq!(
            verdict(
                &[LOGIN, ("host", "PC.Tail1234.TS.NET")],
                Some(" ME@example.com ")
            ),
            "allowed"
        );
    }

    #[test]
    fn tailscale_others_fail() {
        let me = Some("me@example.com");
        assert_eq!(
            verdict(&[LOGIN, ("host", "pc.tail1234.ts.net")], None),
            "login"
        );
        assert_eq!(
            verdict(&[LOGIN, ("host", "pc.tail1234.ts.net")], Some("")),
            "login"
        );
        assert_eq!(
            verdict(
                &[
                    ("tailscale-user-login", "guest@example.com"),
                    ("host", "pc.tail1234.ts.net")
                ],
                me
            ),
            "login"
        );
        assert_eq!(verdict(&[LOGIN, ("host", "127.0.0.1:7420")], me), "origin");
        assert_eq!(verdict(&[LOGIN, ("host", "evil.example")], me), "origin");
        assert_eq!(verdict(&[LOGIN, ("host", ".ts.net")], me), "origin");
        // DNS 재바인딩: 남의 이름에서 같은 출처로 부른 GET(Origin 없음)은 X-Forwarded-Host를 꾸며도 막는다.
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "evil.com"),
                    ("x-forwarded-host", "x.ts.net")
                ],
                me
            ),
            "origin"
        );
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "127.0.0.1:7420"),
                    ("x-forwarded-host", "evil.com")
                ],
                me
            ),
            "origin"
        );
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "pc.tail1234.ts.net"),
                    ("origin", "http://pc.tail1234.ts.net")
                ],
                me
            ),
            "origin"
        );
        assert_eq!(
            verdict(
                &[
                    LOGIN,
                    ("host", "pc.tail1234.ts.net"),
                    ("origin", "https://evil.example")
                ],
                me
            ),
            "origin"
        );
        assert_eq!(verdict(&[LOGIN], me), "origin");
    }
}
