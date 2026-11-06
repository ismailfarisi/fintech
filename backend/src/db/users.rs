use chrono::{DateTime, Utc};
use sqlx::PgExecutor;

use crate::domain::ids::UserId;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub password_hash: String,
    pub failed_login_count: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn insert(
    db: impl PgExecutor<'_>,
    id: UserId,
    email: &str,
    email_normalized: &str,
    display_name: &str,
    password_hash: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO users (id, email, email_normalized, display_name, password_hash) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(email)
    .bind(email_normalized)
    .bind(display_name)
    .bind(password_hash)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn find_by_email(db: impl PgExecutor<'_>, email_normalized: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as(
        "SELECT id, email, display_name, password_hash, failed_login_count, locked_until, disabled_at, created_at
         FROM users WHERE email_normalized = $1",
    )
    .bind(email_normalized)
    .fetch_optional(db)
    .await
}

pub async fn find_by_id(db: impl PgExecutor<'_>, id: UserId) -> sqlx::Result<Option<User>> {
    sqlx::query_as(
        "SELECT id, email, display_name, password_hash, failed_login_count, locked_until, disabled_at, created_at
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Count a failed login; lock the account for `lock_minutes` once `max_failures` is reached.
pub async fn record_failed_login(
    db: impl PgExecutor<'_>,
    id: UserId,
    max_failures: i32,
    lock_minutes: i32,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE users SET
            failed_login_count = failed_login_count + 1,
            locked_until = CASE WHEN failed_login_count + 1 >= $2
                                THEN now() + make_interval(mins => $3) ELSE locked_until END,
            updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(max_failures)
    .bind(lock_minutes)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn reset_failed_logins(db: impl PgExecutor<'_>, id: UserId) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET failed_login_count = 0, locked_until = NULL, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_password(db: impl PgExecutor<'_>, id: UserId, password_hash: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(password_hash)
        .execute(db)
        .await?;
    Ok(())
}
