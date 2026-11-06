use sqlx::PgExecutor;

use crate::domain::ids::UserId;

pub async fn list(db: impl PgExecutor<'_>, user_id: UserId) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT symbol FROM watchlist WHERE user_id = $1 ORDER BY created_at")
        .bind(user_id)
        .fetch_all(db)
        .await
}

pub async fn add(db: impl PgExecutor<'_>, user_id: UserId, symbol: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO watchlist (user_id, symbol) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(user_id)
        .bind(symbol)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn remove(db: impl PgExecutor<'_>, user_id: UserId, symbol: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM watchlist WHERE user_id = $1 AND symbol = $2")
        .bind(user_id)
        .bind(symbol)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn count(db: impl PgExecutor<'_>, user_id: UserId) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM watchlist WHERE user_id = $1").bind(user_id).fetch_one(db).await
}
