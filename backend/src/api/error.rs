//! One error shape for every API failure:
//! `{"error": {"code": "insufficient_funds", "message": "..."}}`.
//! Internal errors are logged with detail and returned as a generic 500.

use std::time::Duration;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::domain::DomainError;
use crate::services::ServiceError;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    retry_after: Option<Duration>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), retry_after: None }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub fn unauthenticated() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in to continue")
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what} not found"))
    }
    pub fn rate_limited(retry_after: Duration) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many requests; slow down")
        }
    }
    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "something went wrong; please try again")
    }
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": { "code": self.code, "message": self.message } }));
        let mut res = (self.status, body).into_response();
        if let Some(d) = self.retry_after {
            res.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(d.as_secs().max(1)));
        }
        res
    }
}

fn domain_code(e: &DomainError) -> &'static str {
    use DomainError::*;
    match e {
        InvalidQuantity | QuantityIncrement(_) | QuantityTooLarge => "invalid_quantity",
        InvalidLimitPrice | UnexpectedLimitPrice | PriceTooLarge => "invalid_price",
        UnknownSymbol(_) => "unknown_symbol",
        NoPrice(_) => "no_price",
        InsufficientFunds { .. } => "insufficient_funds",
        InsufficientQuantity { .. } => "insufficient_quantity",
        InvalidAmount | AmountTooLarge => "invalid_amount",
    }
}

impl From<ServiceError> for ApiError {
    fn from(e: ServiceError) -> Self {
        use ServiceError::*;
        let msg = e.to_string();
        match e {
            Domain(DomainError::UnknownSymbol(_)) => ApiError::new(StatusCode::NOT_FOUND, "unknown_symbol", msg),
            Domain(d) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, domain_code(&d), msg),
            Validation(_) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "validation_error", msg),
            NotFound(_) => ApiError::new(StatusCode::NOT_FOUND, "not_found", msg),
            Conflict(_) => ApiError::new(StatusCode::CONFLICT, "conflict", msg),
            InvalidCredentials => ApiError::new(StatusCode::UNAUTHORIZED, "invalid_credentials", msg),
            Unauthenticated => ApiError::unauthenticated(),
            Forbidden(_) => ApiError::forbidden(msg),
            RateLimited(d) => {
                ApiError { retry_after: Some(d), ..ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", msg) }
            }
            Internal(err) => {
                tracing::error!(error = ?err, "internal error");
                ApiError::internal()
            }
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ServiceError::from(e).into()
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        // body_text() explains the problem (e.g. unknown field) without internals.
        ApiError::new(r.status(), "bad_request", r.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        ApiError::bad_request(r.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(r: PathRejection) -> Self {
        ApiError::bad_request(r.body_text())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
