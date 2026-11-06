//! Deposits and withdrawals (paper money).

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;

use super::{ServiceError, ServiceResult};
use crate::db::accounts;
use crate::db::audit::{self, Actor};
use crate::db::ledger::{self, Book, EntryKind, Posting};
use crate::domain::DomainError;
use crate::domain::ids::{AccountId, EntryId};
use crate::domain::money::CASH;
use crate::domain::order::validate_cash_amount;
use crate::events::{AccountChange, Event};

pub(super) async fn deposit_in_tx(
    conn: &mut PgConnection,
    account_id: AccountId,
    amount: Decimal,
    description: &str,
    actor: &Actor,
) -> ServiceResult<EntryId> {
    let amount = validate_cash_amount(amount)?;
    let entry = ledger::post(
        &mut *conn,
        account_id,
        EntryKind::Deposit,
        None,
        description,
        &[Posting::new(Book::Account, CASH, amount), Posting::new(Book::External, CASH, -amount)],
    )
    .await?;
    audit::record(&mut *conn, actor, Some(account_id), "cash.deposit", json!({ "amount": amount, "entry_id": entry }))
        .await?;
    Ok(entry)
}

/// Deposit into an account the caller has already authorized.
pub async fn deposit(
    conn: &mut PgConnection,
    account_id: AccountId,
    amount: Decimal,
    actor: &Actor,
) -> ServiceResult<(EntryId, Vec<Event>)> {
    accounts::lock_open(&mut *conn, account_id).await?.ok_or(ServiceError::Conflict("account is closed".into()))?;
    let entry = deposit_in_tx(conn, account_id, amount, "Deposit", actor).await?;
    Ok((entry, vec![Event::Account { account_id, change: AccountChange::CashMoved }]))
}

pub async fn withdraw(
    conn: &mut PgConnection,
    account_id: AccountId,
    amount: Decimal,
    actor: &Actor,
) -> ServiceResult<(EntryId, Vec<Event>)> {
    let amount = validate_cash_amount(amount)?;
    accounts::lock_open(&mut *conn, account_id).await?.ok_or(ServiceError::Conflict("account is closed".into()))?;
    let holdings = ledger::lock_holdings(&mut *conn, account_id, &[CASH]).await?;
    let usd = &holdings[0];
    let available = usd.quantity - usd.reserved;
    if amount > available {
        return Err(DomainError::InsufficientFunds { needed: amount, available }.into());
    }
    let entry = ledger::post(
        &mut *conn,
        account_id,
        EntryKind::Withdrawal,
        None,
        "Withdrawal",
        &[Posting::new(Book::Account, CASH, -amount), Posting::new(Book::External, CASH, amount)],
    )
    .await?;
    audit::record(
        &mut *conn,
        actor,
        Some(account_id),
        "cash.withdrawal",
        json!({ "amount": amount, "entry_id": entry }),
    )
    .await?;
    Ok((entry, vec![Event::Account { account_id, change: AccountChange::CashMoved }]))
}
