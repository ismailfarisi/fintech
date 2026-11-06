//! Request-level hardening applied to every route.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::error::ApiError;
use crate::app::SharedState;

/// The caller's IP. `X-Forwarded-For` is honoured only when explicitly
/// configured (otherwise any client could spoof it to dodge rate limits).
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>, trust_proxy: bool) -> IpAddr {
    if trust_proxy {
        // The right-most entry was added by our own proxy; earlier ones are client-controlled.
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return ip;
        }
    }
    peer.map(|p| p.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

pub fn request_ip(req: &Request, trust_proxy: bool) -> IpAddr {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    client_ip(req.headers(), peer, trust_proxy)
}

/// Origin of a request from `Origin`, falling back to `Referer`.
fn request_origin(headers: &HeaderMap) -> Option<String> {
    if let Some(o) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        return Some(o.to_ascii_lowercase());
    }
    let referer = headers.get(header::REFERER)?.to_str().ok()?;
    let (scheme, rest) = referer.split_once("://")?;
    let host = rest.split('/').next()?;
    Some(format!("{scheme}://{host}").to_ascii_lowercase())
}

/// CSRF defence for cookie-authenticated requests, layered on top of
/// `SameSite=Strict` cookies: state-changing requests must come from an
/// allowed origin. Requests with no Origin/Referer *and* no session cookie
/// (non-browser clients) are let through, since there is nothing to forge.
pub async fn csrf_origin_check(State(state): State<SharedState>, req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe {
        let cookie_name = state.config.session_cookie_name();
        let has_session_cookie = req
            .headers()
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|c| c.split(';').any(|kv| kv.trim().starts_with(&format!("{cookie_name}="))));
        // Ambiguous requests (several Origin headers) are refused outright.
        if req.headers().get_all(header::ORIGIN).iter().count() > 1 {
            return ApiError::forbidden("multiple Origin headers").into_response();
        }
        match request_origin(req.headers()) {
            Some(origin) if state.config.allowed_origins.contains(&origin) => {}
            Some(origin) => {
                tracing::warn!(%origin, "blocked cross-origin state-changing request");
                return ApiError::forbidden("cross-origin request blocked").into_response();
            }
            None if has_session_cookie => {
                return ApiError::forbidden("missing Origin header").into_response();
            }
            None => {}
        }
    }
    next.run(req).await
}

/// Per-IP request rate limit for the API.
pub async fn rate_limit(State(state): State<SharedState>, req: Request, next: Next) -> Response {
    let ip = request_ip(&req, state.config.trust_proxy_headers);
    if let Err(retry) = state.limits.api.check(&ip) {
        return ApiError::rate_limited(retry).into_response();
    }
    next.run(req).await
}

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
    connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
    frame-ancestors 'none'";

/// Security headers on every response; `no-store` on API responses so
/// financial data isn't cached by browsers or intermediaries.
pub async fn security_headers(State(state): State<SharedState>, req: Request, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let mut set = |name: HeaderName, value: &'static str| {
        h.entry(name).or_insert(HeaderValue::from_static(value));
    };
    set(header::CONTENT_SECURITY_POLICY, CSP);
    set(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(header::X_FRAME_OPTIONS, "DENY");
    set(header::REFERRER_POLICY, "same-origin");
    set(HeaderName::from_static("permissions-policy"), "camera=(), microphone=(), geolocation=(), payment=(), usb=()");
    set(HeaderName::from_static("cross-origin-opener-policy"), "same-origin");
    set(HeaderName::from_static("cross-origin-resource-policy"), "same-origin");
    if state.config.cookie_secure {
        set(header::STRICT_TRANSPORT_SECURITY, "max-age=63072000; includeSubDomains");
    }
    if is_api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_for_is_ignored_unless_trusted() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "6.6.6.6, 10.0.0.9".parse().unwrap());
        let peer: SocketAddr = "192.0.2.1:5000".parse().unwrap();
        assert_eq!(client_ip(&h, Some(peer), false), "192.0.2.1".parse::<IpAddr>().unwrap());
        assert_eq!(client_ip(&h, Some(peer), true), "10.0.0.9".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn origin_from_referer() {
        let mut h = HeaderMap::new();
        h.insert(header::REFERER, "https://App.example.com/trade?x=1".parse().unwrap());
        assert_eq!(request_origin(&h).as_deref(), Some("https://app.example.com"));
    }
}
