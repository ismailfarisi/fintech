//! Registration, login, sessions and password changes.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{ServiceError, ServiceResult, accounts};
use crate::app::AppState;
use crate::db::audit::{self, Actor};
use crate::db::{self, sessions, users};
use crate::domain::ids::{SessionId, UserId};
use crate::security::{password, token};

/// Lock an account after this many consecutive failed logins...
const MAX_FAILED_LOGINS: i32 = 5;
/// ...for this long.
const LOCK_MINUTES: i32 = 15;
/// Don't write `last_seen_at` on every request.
const TOUCH_INTERVAL_SECS: i64 = 60;

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user: users::User,
    pub session_id: SessionId,
}

pub struct NewSession {
    pub token: String,
    pub session_id: SessionId,
    pub expires_at: DateTime<Utc>,
}

pub struct RegisterInput {
    pub email: String,
    pub password: String,
    pub display_name: String,
}

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn validate_email(email: &str) -> ServiceResult<()> {
    let ok = email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && matches!(email.split_once('@'), Some((local, domain))
            if !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !domain.contains('@'));
    if ok { Ok(()) } else { Err(ServiceError::Validation("enter a valid email address".into())) }
}

fn validate_display_name(name: &str) -> ServiceResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(ServiceError::Validation("name must be 1–100 characters".into()));
    }
    Ok(name.to_string())
}

async fn create_session(state: &AppState, user_id: UserId, actor: &Actor) -> ServiceResult<NewSession> {
    let t = token::generate();
    let session_id = SessionId::new();
    let expires_at = Utc::now() + chrono::Duration::from_std(state.config.session_absolute).expect("duration");
    sessions::insert(
        &state.db,
        session_id,
        user_id,
        &t.hash,
        expires_at,
        actor.ip.as_deref().unwrap_or_default(),
        actor.user_agent.as_deref(),
    )
    .await?;
    Ok(NewSession { token: t.token, session_id, expires_at })
}

fn check_auth_rate_limits(state: &AppState, actor: &Actor, email: Option<&str>) -> ServiceResult<()> {
    if let Some(ip) = actor.ip.as_deref().and_then(|ip| ip.parse().ok()) {
        state.limits.auth_ip.check(&ip).map_err(ServiceError::RateLimited)?;
    }
    if let Some(email) = email {
        state.limits.auth_email.check(&email.to_string()).map_err(ServiceError::RateLimited)?;
    }
    Ok(())
}

pub async fn register(
    state: &AppState,
    input: RegisterInput,
    actor: &Actor,
) -> ServiceResult<(users::User, NewSession)> {
    if !state.config.allow_registration {
        return Err(ServiceError::Forbidden("registration is disabled"));
    }
    check_auth_rate_limits(state, actor, None)?;
    let email = input.email.trim().to_string();
    validate_email(&email)?;
    let email_normalized = normalize_email(&email);
    let display_name = validate_display_name(&input.display_name)?;
    password::check_policy(&input.password, &email).map_err(|m| ServiceError::Validation(m.into()))?;
    let hash = password::hash(input.password).await?;

    let user_id = UserId::new();
    let mut tx = state.db.begin().await?;
    if let Err(e) = users::insert(&mut *tx, user_id, &email, &email_normalized, &display_name, &hash).await {
        if db::is_unique_violation(&e) {
            return Err(ServiceError::Conflict("an account with this email already exists".into()));
        }
        return Err(e.into());
    }
    let actor = Actor { user_id: Some(user_id), ..actor.clone() };
    audit::record(&mut *tx, &actor, None, "user.registered", json!({ "email": email })).await?;
    accounts::open_paper_account(&mut tx, user_id, "Paper account", state.config.starting_cash, &actor).await?;
    tx.commit().await?;

    let session = create_session(state, user_id, &actor).await?;
    audit::record(
        &state.db,
        &actor,
        None,
        "auth.login",
        json!({ "session_id": session.session_id, "via": "register" }),
    )
    .await?;
    let user = users::find_by_id(&state.db, user_id).await?.ok_or(ServiceError::NotFound("user"))?;
    Ok((user, session))
}

pub async fn login(
    state: &AppState,
    email: &str,
    password_input: String,
    actor: &Actor,
) -> ServiceResult<(users::User, NewSession)> {
    let email_normalized = normalize_email(email);
    // No real account has an email this long; don't let it become a limiter key.
    if email_normalized.len() > 254 {
        check_auth_rate_limits(state, actor, None)?;
        return Err(ServiceError::InvalidCredentials);
    }
    check_auth_rate_limits(state, actor, Some(&email_normalized))?;
    let user = users::find_by_email(&state.db, &email_normalized).await?;

    if let Some(u) = &user
        && u.locked_until.is_some_and(|t| t > Utc::now())
    {
        let actor = Actor { user_id: Some(u.id), ..actor.clone() };
        audit::record(&state.db, &actor, None, "auth.login_blocked", json!({ "reason": "locked" })).await?;
        return Err(ServiceError::RateLimited(std::time::Duration::from_secs(60 * LOCK_MINUTES as u64)));
    }

    // Always run a full Argon2 verification, even for unknown emails.
    let ok = password::verify(password_input, user.as_ref().map(|u| u.password_hash.clone())).await;
    let user = match (ok, user) {
        (true, Some(u)) if u.disabled_at.is_none() => u,
        (_, u) => {
            if let Some(u) = &u {
                users::record_failed_login(&state.db, u.id, MAX_FAILED_LOGINS, LOCK_MINUTES).await?;
            }
            let actor = Actor { user_id: u.as_ref().map(|u| u.id), ..actor.clone() };
            audit::record(&state.db, &actor, None, "auth.login_failed", json!({ "email": email_normalized })).await?;
            return Err(ServiceError::InvalidCredentials);
        }
    };
    users::reset_failed_logins(&state.db, user.id).await?;
    let actor = Actor { user_id: Some(user.id), ..actor.clone() };
    let session = create_session(state, user.id, &actor).await?;
    audit::record(&state.db, &actor, None, "auth.login", json!({ "session_id": session.session_id })).await?;
    Ok((user, session))
}

/// Resolve a session token from a cookie. `Ok(None)` for any invalid token.
pub async fn authenticate(state: &AppState, raw_token: &str) -> ServiceResult<Option<AuthContext>> {
    if !token::looks_valid(raw_token) {
        return Ok(None);
    }
    let idle = state.config.session_idle.as_secs() as i64;
    let Some(session) = sessions::find_active(&state.db, &token::hash(raw_token), idle).await? else {
        return Ok(None);
    };
    if (Utc::now() - session.last_seen_at).num_seconds() > TOUCH_INTERVAL_SECS {
        sessions::touch(&state.db, session.id).await?;
    }
    let Some(user) = users::find_by_id(&state.db, session.user_id).await? else {
        return Ok(None);
    };
    Ok(Some(AuthContext { user, session_id: session.id }))
}

pub async fn logout(state: &AppState, ctx: &AuthContext, actor: &Actor) -> ServiceResult<()> {
    sessions::revoke(&state.db, ctx.user.id, ctx.session_id).await?;
    audit::record(&state.db, actor, None, "auth.logout", json!({ "session_id": ctx.session_id })).await?;
    Ok(())
}

pub async fn change_password(
    state: &AppState,
    ctx: &AuthContext,
    current: String,
    new: String,
    actor: &Actor,
) -> ServiceResult<u64> {
    check_auth_rate_limits(state, actor, Some(&normalize_email(&ctx.user.email)))?;
    if !password::verify(current, Some(ctx.user.password_hash.clone())).await {
        audit::record(&state.db, actor, None, "auth.password_change_failed", json!({})).await?;
        return Err(ServiceError::InvalidCredentials);
    }
    password::check_policy(&new, &ctx.user.email).map_err(|m| ServiceError::Validation(m.into()))?;
    let hash = password::hash(new).await?;
    let mut tx = state.db.begin().await?;
    users::update_password(&mut *tx, ctx.user.id, &hash).await?;
    // A password change signs out every other device.
    let revoked = sessions::revoke_all(&mut *tx, ctx.user.id, Some(ctx.session_id)).await?;
    audit::record(&mut *tx, actor, None, "auth.password_changed", json!({ "other_sessions_revoked": revoked })).await?;
    tx.commit().await?;
    Ok(revoked)
}

pub async fn list_sessions(state: &AppState, ctx: &AuthContext) -> ServiceResult<Vec<sessions::Session>> {
    Ok(sessions::list_active(&state.db, ctx.user.id, state.config.session_idle.as_secs() as i64).await?)
}

pub async fn revoke_session(state: &AppState, ctx: &AuthContext, id: SessionId, actor: &Actor) -> ServiceResult<()> {
    if !sessions::revoke(&state.db, ctx.user.id, id).await? {
        return Err(ServiceError::NotFound("session"));
    }
    audit::record(&state.db, actor, None, "auth.session_revoked", json!({ "session_id": id })).await?;
    Ok(())
}

pub async fn revoke_other_sessions(state: &AppState, ctx: &AuthContext, actor: &Actor) -> ServiceResult<u64> {
    let n = sessions::revoke_all(&state.db, ctx.user.id, Some(ctx.session_id)).await?;
    audit::record(&state.db, actor, None, "auth.sessions_revoked", json!({ "count": n })).await?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation() {
        for ok in ["a@b.co", "first.last+tag@example.com"] {
            assert!(validate_email(ok).is_ok(), "{ok}");
        }
        for bad in ["", "no-at", "@x.com", "a@nodot", "a b@c.com", "a@.com", "a@b.com.", "a@b@c.com"] {
            assert!(validate_email(bad).is_err(), "{bad}");
        }
        assert_eq!(normalize_email("  Alice@Example.COM "), "alice@example.com");
    }
}
