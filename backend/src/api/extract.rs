//! Request extractors: JSON bodies with uniform errors, request metadata,
//! and the authenticated user.

use axum::extract::{FromRequest, FromRequestParts};
use axum::http::header;
use axum::http::request::Parts;

use super::error::ApiError;
use crate::app::SharedState;
use crate::db::audit::Actor;
use crate::security::middleware::client_ip;
use crate::services::auth::{self, AuthContext};

/// `axum::Json`, with rejections mapped to [`ApiError`].
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct Json<T>(pub T);

impl<T: serde::Serialize> axum::response::IntoResponse for Json<T> {
    fn into_response(self) -> axum::response::Response {
        axum::Json(self.0).into_response()
    }
}

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

/// Who is calling, for audit records.
#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub ip: String,
    pub user_agent: Option<String>,
}

impl RequestMeta {
    pub fn actor(&self, ctx: Option<&AuthContext>) -> Actor {
        Actor { user_id: ctx.map(|c| c.user.id), ip: Some(self.ip.clone()), user_agent: self.user_agent.clone() }
    }
}

impl FromRequestParts<SharedState> for RequestMeta {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0);
        let ip = client_ip(&parts.headers, peer, state.config.trust_proxy_headers).to_string();
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|ua| ua.chars().take(256).collect());
        Ok(RequestMeta { ip, user_agent })
    }
}

/// Read a cookie value by name.
pub fn cookie<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    cookie_from_headers(&parts.headers, name)
}

pub fn cookie_from_headers<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The signed-in user. Rejects with 401 if there is no valid session.
#[derive(Debug, Clone)]
pub struct Auth(pub AuthContext);

impl FromRequestParts<SharedState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, Self::Rejection> {
        if let Some(ctx) = parts.extensions.get::<AuthContext>() {
            return Ok(Auth(ctx.clone()));
        }
        let token = cookie(parts, state.config.session_cookie_name()).ok_or_else(ApiError::unauthenticated)?;
        let ctx = auth::authenticate(state, token).await?.ok_or_else(ApiError::unauthenticated)?;
        parts.extensions.insert(ctx.clone());
        Ok(Auth(ctx))
    }
}
