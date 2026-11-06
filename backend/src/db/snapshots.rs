use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgExecutor;

use crate::domain::ids::AccountId;

pub async fn insert_many(
    db: impl PgExecutor<'_>,
    at: DateTime<Utc>,
    rows: &[(AccountId, Decimal)],
) -> sqlx::Result<()> {
    let ids: Vec<uuid::Uuid> = rows.iter().map(|r| r.0.0).collect();
    let equity: Vec<Decimal> = rows.iter().map(|r| r.1).collect();
    sqlx::query(
        "INSERT INTO equity_snapshots (account_id, at, equity)
         SELECT unnest($1::uuid[]), $2, unnest($3::numeric[]) ON CONFLICT DO NOTHING",
    )
    .bind(&ids)
    .bind(at)
    .bind(&equity)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct EquityPoint {
    pub at: DateTime<Utc>,
    pub equity: Decimal,
}

pub async fn list(
    db: impl PgExecutor<'_>,
    account_id: AccountId,
    since: DateTime<Utc>,
) -> sqlx::Result<Vec<EquityPoint>> {
    sqlx::query_as("SELECT at, equity FROM equity_snapshots WHERE account_id = $1 AND at >= $2 ORDER BY at LIMIT 5000")
        .bind(account_id)
        .bind(since)
        .fetch_all(db)
        .await
}
