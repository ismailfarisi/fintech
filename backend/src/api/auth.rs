//! /api/v1/auth: registration, login, sessions, password.

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::ApiResult;
use super::extract::{ApiPath, Auth, Json, RequestMeta};
use crate::app::{AppState, SharedState};
use crate::config::Config;
use crate::db::{accounts, audit, users};
use crate::domain::ids::{SessionId, UserId};
use crate::services::auth::{self, NewSession, RegisterInput};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/password", post(change_password))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", delete(revoke_session))
        .route("/sessions/revoke-others", post(revoke_others))
        .route("/activity", get(activity))
}

/// Passwords are only ever held in this type, which never prints its contents.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct Password(String);

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password([redacted])")
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterBody {
    email: String,
    password: Password,
    display_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    email: String,
    password: Password,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordBody {
    current_password: Password,
    new_password: Password,
}

#[derive(Serialize)]
pub struct UserView {
    id: UserId,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
}

impl From<&users::User> for UserView {
    fn from(u: &users::User) -> Self {
        Self { id: u.id, email: u.email.clone(), display_name: u.display_name.clone(), created_at: u.created_at }
    }
}

#[derive(Serialize)]
struct MeView {
    user: UserView,
    accounts: Vec<accounts::Account>,
}

/// Session cookie: HttpOnly (no JS access), SameSite=Strict (not sent on
/// cross-site requests), Secure + `__Host-` prefix when enabled.
fn session_cookie(config: &Config, value: &str, max_age_secs: i64) -> HeaderValue {
    let mut c =
        format!("{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}", config.session_cookie_name());
    if config.cookie_secure {
        c.push_str("; Secure");
    }
    HeaderValue::from_str(&c).expect("cookie is ASCII")
}

fn with_session(state: &AppState, status: StatusCode, body: impl Serialize, s: &NewSession) -> Response {
    let max_age = (s.expires_at - Utc::now()).num_seconds().max(0);
    let mut res = (status, axum::Json(body)).into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie(&state.config, &s.token, max_age));
    res
}

async fn me_view(state: &AppState, user: &users::User) -> ApiResult<MeView> {
    Ok(MeView { user: user.into(), accounts: accounts::list_for_user(&state.db, user.id).await? })
}

async fn register(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Json(b): Json<RegisterBody>,
) -> ApiResult<Response> {
    let input = RegisterInput { email: b.email, password: b.password.0, display_name: b.display_name };
    let (user, session) = auth::register(&state, input, &meta.actor(None)).await?;
    let view = me_view(&state, &user).await?;
    Ok(with_session(&state, StatusCode::CREATED, view, &session))
}

async fn login(State(state): State<SharedState>, meta: RequestMeta, Json(b): Json<LoginBody>) -> ApiResult<Response> {
    let (user, session) = auth::login(&state, &b.email, b.password.0, &meta.actor(None)).await?;
    let view = me_view(&state, &user).await?;
    Ok(with_session(&state, StatusCode::OK, view, &session))
}

async fn logout(State(state): State<SharedState>, meta: RequestMeta, Auth(ctx): Auth) -> ApiResult<Response> {
    auth::logout(&state, &ctx, &meta.actor(Some(&ctx))).await?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie(&state.config, "", 0));
    Ok(res)
}

async fn me(State(state): State<SharedState>, Auth(ctx): Auth) -> ApiResult<Json<MeView>> {
    Ok(Json(me_view(&state, &ctx.user).await?))
}

async fn change_password(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    Json(b): Json<PasswordBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let revoked =
        auth::change_password(&state, &ctx, b.current_password.0, b.new_password.0, &meta.actor(Some(&ctx))).await?;
    Ok(Json(serde_json::json!({ "other_sessions_revoked": revoked })))
}

#[derive(Serialize)]
struct SessionView {
    id: SessionId,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    ip: Option<String>,
    user_agent: Option<String>,
    current: bool,
}

async fn list_sessions(State(state): State<SharedState>, Auth(ctx): Auth) -> ApiResult<Json<Vec<SessionView>>> {
    let sessions = auth::list_sessions(&state, &ctx).await?;
    Ok(Json(
        sessions
            .into_iter()
            .map(|s| SessionView {
                current: s.id == ctx.session_id,
                id: s.id,
                created_at: s.created_at,
                last_seen_at: s.last_seen_at,
                expires_at: s.expires_at,
                ip: s.ip,
                user_agent: s.user_agent,
            })
            .collect(),
    ))
}

async fn revoke_session(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<SessionId>,
) -> ApiResult<StatusCode> {
    auth::revoke_session(&state, &ctx, id, &meta.actor(Some(&ctx))).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke_others(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
) -> ApiResult<Json<serde_json::Value>> {
    let n = auth::revoke_other_sessions(&state, &ctx, &meta.actor(Some(&ctx))).await?;
    Ok(Json(serde_json::json!({ "revoked": n })))
}

/// The user's own security & money activity log.
async fn activity(State(state): State<SharedState>, Auth(ctx): Auth) -> ApiResult<Json<Vec<audit::AuditEntry>>> {
    Ok(Json(audit::recent_for_user(&state.db, ctx.user.id, 100).await?))
}
