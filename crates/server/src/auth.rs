//! Password authentication for the HTTP API and the dashboard session cookie.

use axum::{
    Json,
    extract::{Request, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use crate::state::SharedState;

pub const SESSION_COOKIE: &str = "me_session";

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub password: String,
}

/// Middleware that accepts either `Authorization: Bearer <password>` or the
/// dashboard session cookie.
pub async fn require_auth(
    State(state): State<SharedState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = state.config.password.as_deref() else {
        return next.run(request).await;
    };

    let supplied =
        bearer_token(request.headers()).or_else(|| cookie_value(request.headers(), SESSION_COOKIE));

    match supplied {
        Some(candidate) if constant_time_eq(candidate.as_bytes(), expected.as_bytes()) => {
            next.run(request).await
        }
        _ => unauthorized("authentication required"),
    }
}

/// Authenticate a dashboard password and set an HttpOnly session cookie.
pub async fn login(
    State(state): State<SharedState>,
    body: Result<Json<LoginBody>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "failed",
                    "error": {"code": "INVALID_INPUT", "message": rejection.body_text()},
                })),
            )
                .into_response();
        }
    };

    let Some(expected) = state.config.password.as_deref() else {
        return (
            [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
            Json(json!({"status": "succeeded", "authenticated": true, "auth": "disabled"})),
        )
            .into_response();
    };

    if !constant_time_eq(body.password.trim().as_bytes(), expected.as_bytes()) {
        // Slow down online guessing without making the endpoint a timing oracle.
        tokio::time::sleep(Duration::from_millis(350)).await;
        return unauthorized("invalid password");
    }

    let cookie = session_cookie(
        expected,
        state.config.session_ttl_secs,
        state.config.secure_cookie,
    );
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie).expect("valid cookie"),
            ),
        ],
        Json(json!({
            "status": "succeeded",
            "authenticated": true,
            "expires_in": state.config.session_ttl_secs,
        })),
    )
        .into_response()
}

/// Clear the dashboard session cookie.
pub async fn logout() -> Response {
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie).expect("valid cookie"),
            ),
        ],
        Json(json!({"status": "succeeded", "authenticated": false})),
    )
        .into_response()
}

/// Return session metadata for the already-authenticated dashboard.
pub async fn session(State(state): State<SharedState>) -> Response {
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(json!({
            "status": "succeeded",
            "authenticated": true,
            "auth": if state.config.auth_required() { "password" } else { "disabled" },
            "server": "me-server",
            "version": env!("CARGO_PKG_VERSION"),
            "started_at": state.started_at.to_rfc3339(),
        })),
    )
        .into_response()
}

pub fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"me-server\""),
        )],
        Json(json!({
            "status": "failed",
            "error": {"code": "UNAUTHORIZED", "message": message},
        })),
    )
        .into_response()
}

fn session_cookie(password: &str, ttl_seconds: u64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE}={password}; HttpOnly; SameSite=Strict; Path=/; Max-Age={ttl_seconds}{secure}"
    )
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        if let Some((key, value)) = part.trim().split_once('=')
            && key == name
        {
            return Some(value.to_string());
        }
    }
    None
}

/// Compare two byte strings without early exit on the first differing byte.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_comparison_matches_normal_equality() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreu"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(!constant_time_eq(b"", b"x"));
    }
}
