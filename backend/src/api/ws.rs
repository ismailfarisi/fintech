//! /api/v1/ws?account_id=...: live quotes + portfolio for one account.
//!
//! Security: requires a valid session cookie, an allowed `Origin` (browsers
//! don't apply CORS to WebSockets, so this blocks cross-site hijacking), and
//! ownership of the account. The session is re-checked every minute.

use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::error::{ApiError, ApiResult};
use super::extract::{ApiQuery, Auth};
use crate::app::SharedState;
use crate::domain::ids::AccountId;
use crate::events::Event;
use crate::services::accounts::get_owned;
use crate::services::auth;
use crate::services::portfolio::{AccountBook, prices};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WsQuery {
    account_id: AccountId,
}

pub async fn handler(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    headers: HeaderMap,
    ApiQuery(q): ApiQuery<WsQuery>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_ascii_lowercase);
    if !origin.is_some_and(|o| state.config.allowed_origins.contains(&o)) {
        return Err(ApiError::forbidden("websocket origin not allowed"));
    }
    get_owned(&state.db, ctx.user.id, q.account_id).await?;
    let token = super::extract::cookie_from_headers(&headers, state.config.session_cookie_name())
        .unwrap_or_default()
        .to_string();
    Ok(ws.max_message_size(4096).on_upgrade(move |socket| session(socket, state, token, q.account_id)))
}

async fn session(mut socket: WebSocket, state: SharedState, token: String, account_id: AccountId) {
    let mut rx = state.events.subscribe();
    let mut book = match AccountBook::load(&state.db, account_id).await {
        Ok(b) => b,
        Err(_) => return,
    };
    let mut recheck = tokio::time::interval(Duration::from_secs(60));
    recheck.tick().await;
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let msg = match ev {
                    Ok(Event::Quotes(quotes)) => {
                        let portfolio = book.value(&prices(&state));
                        json!({ "type": "tick", "quotes": &*quotes, "portfolio": portfolio })
                    }
                    Ok(Event::Account { account_id: a, change }) if a == account_id => {
                        if let Ok(b) = AccountBook::load(&state.db, account_id).await {
                            book = b;
                        }
                        json!({ "type": "account", "change": change })
                    }
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            _ = recheck.tick() => {
                // Logout / revocation / expiry ends the stream.
                if !matches!(auth::authenticate(&state, &token).await, Ok(Some(_))) {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {} // client messages are ignored
            },
        }
    }
}
