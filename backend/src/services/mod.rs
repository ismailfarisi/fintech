//! Application services: use-cases that combine domain rules with
//! repositories inside database transactions.
//!
//! Money-moving services take `&mut PgConnection` (an open transaction) so the
//! caller can compose them atomically, e.g. with an idempotency record.
//! They return events to publish *after* commit.

pub mod accounts;
pub mod auth;
pub mod cash;
pub mod portfolio;
pub mod trading;

use std::time::Duration;

use thiserror::Error;

use crate::domain::DomainError;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("{0}")]
    Validation(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(String),
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error("authentication required")]
    Unauthenticated,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("too many attempts; try again later")]
    RateLimited(Duration),
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ServiceError {
    fn from(e: sqlx::Error) -> Self {
        // A CHECK violation means a balance would have gone negative: the
        // database caught what validation should have. Never expose details.
        if crate::db::is_check_violation(&e) {
            tracing::error!("constraint violation reached the database: {e}");
            return ServiceError::Conflict("the request conflicts with the current account state; please retry".into());
        }
        ServiceError::Internal(e.into())
    }
}

pub type ServiceResult<T> = Result<T, ServiceError>;
