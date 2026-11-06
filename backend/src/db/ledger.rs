//! Double-entry ledger and holdings.
//!
//! `post` writes an entry with its postings and applies the `account`-book
//! postings to `holdings` in the same transaction. The database rejects the
//! transaction at COMMIT if any asset in the entry doesn't sum to zero, and
//! rejects any holdings update that would go negative.

use rust_decimal::Decimal;
use sqlx::{PgConnection, PgExecutor};

use crate::domain::ids::{AccountId, EntryId};
use crate::domain::portfolio::Holding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Book {
    Account,
    External,
    Market,
    Fees,
}

impl Book {
    fn as_str(self) -> &'static str {
        match self {
            Book::Account => "account",
            Book::External => "external",
            Book::Market => "market",
            Book::Fees => "fees",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Posting {
    pub book: Book,
    pub asset: String,
    pub amount: Decimal,
}

impl Posting {
    pub fn new(book: Book, asset: impl Into<String>, amount: Decimal) -> Self {
        Self { book, asset: asset.into(), amount }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum EntryKind {
    Deposit,
    Withdrawal,
    Trade,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            EntryKind::Deposit => "deposit",
            EntryKind::Withdrawal => "withdrawal",
            EntryKind::Trade => "trade",
        }
    }
}

/// Write a balanced entry and update holdings for the account's own book.
pub async fn post(
    conn: &mut PgConnection,
    account_id: AccountId,
    kind: EntryKind,
    reference_id: Option<uuid::Uuid>,
    description: &str,
    postings: &[Posting],
) -> sqlx::Result<EntryId> {
    let entry_id = EntryId::new();
    sqlx::query(
        "INSERT INTO ledger_entries (id, account_id, kind, reference_id, description) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(entry_id)
    .bind(account_id)
    .bind(kind.as_str())
    .bind(reference_id)
    .bind(description)
    .execute(&mut *conn)
    .await?;
    for p in postings.iter().filter(|p| !p.amount.is_zero()) {
        sqlx::query("INSERT INTO postings (entry_id, account_id, book, asset, amount) VALUES ($1, $2, $3, $4, $5)")
            .bind(entry_id)
            .bind(account_id)
            .bind(p.book.as_str())
            .bind(&p.asset)
            .bind(p.amount)
            .execute(&mut *conn)
            .await?;
        if p.book == Book::Account {
            adjust(&mut *conn, account_id, &p.asset, p.amount, Decimal::ZERO, Decimal::ZERO).await?;
        }
    }
    Ok(entry_id)
}

/// Create holding rows if missing, then lock them (in a fixed order, to avoid
/// deadlocks between concurrent transactions on the same account).
pub async fn lock_holdings(
    conn: &mut PgConnection,
    account_id: AccountId,
    assets: &[&str],
) -> sqlx::Result<Vec<Holding>> {
    let assets: Vec<String> = assets.iter().map(|a| a.to_string()).collect();
    sqlx::query("INSERT INTO holdings (account_id, asset) SELECT $1, unnest($2::text[]) ON CONFLICT DO NOTHING")
        .bind(account_id)
        .bind(&assets)
        .execute(&mut *conn)
        .await?;
    let rows: Vec<(String, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "SELECT asset, quantity, reserved, cost_basis FROM holdings
         WHERE account_id = $1 AND asset = ANY($2) ORDER BY asset FOR UPDATE",
    )
    .bind(account_id)
    .bind(&assets)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(asset, quantity, reserved, cost_basis)| Holding { asset, quantity, reserved, cost_basis })
        .collect())
}

/// Apply deltas to a holding. CHECK constraints make overdrafts impossible.
///
/// (Not `INSERT .. ON CONFLICT DO UPDATE`: Postgres checks constraints on the
/// proposed *insert* row first, so a pure delta like `reserved = +50` on a
/// zero row would be rejected even though the merged row is valid.)
pub async fn adjust(
    conn: &mut PgConnection,
    account_id: AccountId,
    asset: &str,
    quantity: Decimal,
    reserved: Decimal,
    cost_basis: Decimal,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO holdings (account_id, asset) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(account_id)
        .bind(asset)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE holdings SET
            quantity = quantity + $3,
            reserved = reserved + $4,
            cost_basis = cost_basis + $5,
            updated_at = now()
         WHERE account_id = $1 AND asset = $2",
    )
    .bind(account_id)
    .bind(asset)
    .bind(quantity)
    .bind(reserved)
    .bind(cost_basis)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn holdings(db: impl PgExecutor<'_>, account_id: AccountId) -> sqlx::Result<Vec<Holding>> {
    let rows: Vec<(String, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "SELECT asset, quantity, reserved, cost_basis FROM holdings WHERE account_id = $1 ORDER BY asset",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(asset, quantity, reserved, cost_basis)| Holding { asset, quantity, reserved, cost_basis })
        .collect())
}

/// Holdings of every open account (for equity snapshots).
pub async fn all_open_holdings(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<(AccountId, Holding)>> {
    let rows: Vec<(AccountId, String, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "SELECT h.account_id, h.asset, h.quantity, h.reserved, h.cost_basis
         FROM holdings h JOIN accounts a ON a.id = h.account_id WHERE a.closed_at IS NULL",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(a, asset, quantity, reserved, cost_basis)| (a, Holding { asset, quantity, reserved, cost_basis }))
        .collect())
}

/// Money deposited minus money withdrawn (the P&L baseline).
pub async fn net_contributions(db: impl PgExecutor<'_>, account_id: AccountId) -> sqlx::Result<Decimal> {
    sqlx::query_scalar(
        "SELECT coalesce(sum(p.amount), 0) FROM postings p JOIN ledger_entries e ON e.id = p.entry_id
         WHERE p.account_id = $1 AND p.book = 'account' AND p.asset = 'USD'
           AND e.kind IN ('deposit', 'withdrawal')",
    )
    .bind(account_id)
    .fetch_one(db)
    .await
}

/// Reconciliation: holdings must equal the sum of account-book postings.
/// Returns the assets that disagree (empty = consistent).
pub async fn reconcile(
    db: impl PgExecutor<'_>,
    account_id: AccountId,
) -> sqlx::Result<Vec<(String, Decimal, Decimal)>> {
    sqlx::query_as(
        "WITH ledger AS (
            SELECT asset, sum(amount) AS total FROM postings
            WHERE account_id = $1 AND book = 'account' GROUP BY asset
         )
         SELECT coalesce(h.asset, l.asset), coalesce(h.quantity, 0), coalesce(l.total, 0)
         FROM holdings h FULL JOIN ledger l ON l.asset = h.asset AND h.account_id = $1
         WHERE (h.account_id = $1 OR h.account_id IS NULL)
           AND coalesce(h.quantity, 0) <> coalesce(l.total, 0)",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct CashMovement {
    pub id: EntryId,
    pub kind: String,
    pub amount: Decimal,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn cash_movements(
    db: impl PgExecutor<'_>,
    account_id: AccountId,
    limit: i64,
) -> sqlx::Result<Vec<CashMovement>> {
    sqlx::query_as(
        "SELECT e.id, e.kind, abs(p.amount) AS amount, e.created_at FROM ledger_entries e
         JOIN postings p ON p.entry_id = e.id AND p.book = 'account' AND p.asset = 'USD'
         WHERE e.account_id = $1 AND e.kind IN ('deposit', 'withdrawal')
         ORDER BY e.created_at DESC LIMIT $2",
    )
    .bind(account_id)
    .bind(limit)
    .fetch_all(db)
    .await
}
