//! Account lifecycle.

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::{PgConnection, PgExecutor};

use super::{ServiceError, ServiceResult, cash, trading};
use crate::app::AppState;
use crate::db::accounts::{self, Account};
use crate::db::audit::{self, Actor};
use crate::db::orders;
use crate::domain::ids::{AccountId, UserId};
use crate::events::{AccountChange, Event};

/// Open a paper account, funded with `starting_cash` via a normal ledger deposit.
pub async fn open_paper_account(
    conn: &mut PgConnection,
    user_id: UserId,
    name: &str,
    starting_cash: Decimal,
    actor: &Actor,
) -> ServiceResult<AccountId> {
    let id = AccountId::new();
    accounts::insert(&mut *conn, id, user_id, name).await?;
    audit::record(&mut *conn, actor, Some(id), "account.opened", json!({ "kind": "paper" })).await?;
    if starting_cash > Decimal::ZERO {
        cash::deposit_in_tx(conn, id, starting_cash, "Starting paper balance", actor).await?;
    }
    Ok(id)
}

/// The authorization gate for every account-scoped request.
pub async fn get_owned(db: impl PgExecutor<'_>, user_id: UserId, id: AccountId) -> ServiceResult<Account> {
    // Not-found and not-yours are indistinguishable to the caller.
    accounts::get_owned(db, user_id, id).await?.ok_or(ServiceError::NotFound("account"))
}

/// Ledger history is immutable, so "reset" means: cancel open orders, close
/// this account (kept for the record), and open a fresh one.
pub async fn reset(
    state: &AppState,
    user_id: UserId,
    id: AccountId,
    actor: &Actor,
) -> ServiceResult<(AccountId, Vec<Event>)> {
    let account = get_owned(&state.db, user_id, id).await?;
    if account.closed_at.is_some() {
        return Err(ServiceError::Conflict("account is already closed".into()));
    }
    let mut tx = state.db.begin().await?;
    accounts::lock_open(&mut *tx, id).await?.ok_or(ServiceError::Conflict("account is already closed".into()))?;
    let mut events = Vec::new();
    for order_id in orders::open_for_account(&mut *tx, id).await? {
        events.extend(trading::cancel_locked(&mut tx, id, order_id, actor).await?.1);
    }
    accounts::close(&mut *tx, id).await?;
    audit::record(&mut *tx, actor, Some(id), "account.closed", json!({ "reason": "reset" })).await?;
    let new_id = open_paper_account(&mut tx, user_id, &account.name, state.config.starting_cash, actor).await?;
    tx.commit().await?;
    events.push(Event::Account { account_id: id, change: AccountChange::Closed });
    Ok((new_id, events))
}
