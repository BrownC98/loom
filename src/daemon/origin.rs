//! 사람이 연 다른 웹 페이지가 데몬을 부르지 못하게 한다.
//!
//! 사람 문에는 토큰이 없다. 브라우저는 JSON 요청을 사전 요청으로 막지만, 두 길은 막지 않는다.
//! - 웹소켓: 출처와 관계없이 연결한다. 다른 페이지가 터미널 웹소켓에 글을 넣을 수 있다.
//! - DNS 재바인딩: 공격자 도메인이 127.0.0.1을 가리키게 바꾸면 같은 출처가 되어 모든 API를 부른다.
//!
//! 그래서 `Host`는 IP 주소나 `localhost`만 받고, `Origin`이 있으면 `http://<Host>`와 같아야 한다.
//! 노드의 `loom` 명령과 Windows 앱(데몬 주소를 그대로 연다)은 이 조건을 늘 지킨다.

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub async fn same_site(request: Request, next: Next) -> Response {
    if allowed(request.headers()) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
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
}
