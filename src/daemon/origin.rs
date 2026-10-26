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
