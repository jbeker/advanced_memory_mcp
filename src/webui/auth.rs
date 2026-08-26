// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Jeremy Beker

//! Cookie-based session auth for the web UI. Port of `webui/auth.py`.
//!
//! Same model: the user pastes their MCP bearer token at /ui/login, we
//! validate it against the shared TokenConfig and issue a signed HttpOnly
//! cookie scoped to /ui. Every request re-resolves the token so revocations
//! take effect immediately. The signing scheme is HMAC-SHA256 rather than
//! Python's itsdangerous serializer — sessions don't survive a swap between
//! implementations, which is acceptable (users just sign in again).
//!
//! CSRF posture matches Python: SameSite=Lax + cookie-only auth, no CSRF
//! token in v1.

use crate::tokens::{TokenConfig, TokenEntry};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub const COOKIE_NAME: &str = "mcp_ui_session";
pub const COOKIE_MAX_AGE: i64 = 86_400; // 24 hours
pub const COOKIE_PATH: &str = "/ui";
const SIGNATURE_SALT: &str = "mcp-ui-session";

#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub token: String,
    pub entry: TokenEntry,
}

fn mac(secret: &str, message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(SIGNATURE_SALT.as_bytes());
    mac.update(b":");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// Build the signed cookie value: `base64(token).timestamp.base64(hmac)`.
pub fn sign_session(secret: &str, token: &str, now_unix: i64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(token.as_bytes());
    let message = format!("{payload}.{now_unix}");
    let sig = URL_SAFE_NO_PAD.encode(mac(secret, message.as_bytes()));
    format!("{message}.{sig}")
}

/// Verify a cookie value and return the embedded token if the signature is
/// valid and the session is within COOKIE_MAX_AGE.
pub fn verify_session(secret: &str, value: &str, now_unix: i64) -> Option<String> {
    let (message, sig_b64) = value.rsplit_once('.')?;
    let sig = URL_SAFE_NO_PAD.decode(sig_b64).ok()?;
    let expected = mac(secret, message.as_bytes());
    if !bool::from(expected.as_slice().ct_eq(sig.as_slice())) {
        return None;
    }
    let (payload_b64, ts_str) = message.rsplit_once('.')?;
    let issued: i64 = ts_str.parse().ok()?;
    if now_unix < issued || now_unix - issued > COOKIE_MAX_AGE {
        return None;
    }
    let token_bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    String::from_utf8(token_bytes).ok()
}

/// Extract the session cookie from a Cookie header value.
pub fn cookie_from_header(header: &str) -> Option<&str> {
    header.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == COOKIE_NAME).then_some(value)
    })
}

/// Resolve the request's session cookie to a CurrentUser. None if the cookie
/// is missing, tampered, expired, or references a revoked token.
pub fn resolve_session(
    cookie_header: Option<&str>,
    secret: &str,
    tokens: &TokenConfig,
) -> Option<CurrentUser> {
    let raw = cookie_from_header(cookie_header?)?;
    let token = verify_session(secret, raw, chrono::Utc::now().timestamp())?;
    let entry = tokens.get(&token).ok()?.clone();
    Some(CurrentUser { token, entry })
}

/// Set-Cookie value for issuing a session.
pub fn issue_cookie_header(secret: &str, token: &str, secure: bool) -> String {
    let value = sign_session(secret, token, chrono::Utc::now().timestamp());
    let mut header = format!(
        "{COOKIE_NAME}={value}; Max-Age={COOKIE_MAX_AGE}; Path={COOKIE_PATH}; HttpOnly; SameSite=Lax"
    );
    if secure {
        header.push_str("; Secure");
    }
    header
}

/// Set-Cookie value for clearing the session.
pub fn clear_cookie_header() -> String {
    format!("{COOKIE_NAME}=; Max-Age=0; Path={COOKIE_PATH}; HttpOnly; SameSite=Lax")
}

/// Show enough of the token to identify the session without leaking it.
/// Tokens are formatted `<username>_<hex>` by generate_token.py.
pub fn user_label(token: &str) -> &str {
    match token.split_once('_') {
        Some((name, _)) => name,
        None => &token[..token.len().min(6)],
    }
}

/// Only allow same-origin paths under /ui/ as post-login redirect targets.
pub fn safe_next(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return "/ui/".to_string();
    };
    // Reject absolute URLs (scheme or authority) and anything outside /ui/.
    if raw.contains("://") || raw.starts_with("//") || !raw.starts_with("/ui/") {
        return "/ui/".to_string();
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip_and_expiry() {
        let now = 1_750_000_000;
        let cookie = sign_session("secret", "alice_abc", now);
        assert_eq!(verify_session("secret", &cookie, now + 100).as_deref(), Some("alice_abc"));
        // expired
        assert_eq!(verify_session("secret", &cookie, now + COOKIE_MAX_AGE + 1), None);
        // wrong secret
        assert_eq!(verify_session("other", &cookie, now + 100), None);
        // tampered
        let tampered = cookie.replace('a', "b");
        assert_eq!(verify_session("secret", &tampered, now + 100), None);
    }

    #[test]
    fn cookie_header_parsing() {
        let header = format!("foo=bar; {COOKIE_NAME}=xyz; other=1");
        assert_eq!(cookie_from_header(&header), Some("xyz"));
        assert_eq!(cookie_from_header("foo=bar"), None);
    }

    #[test]
    fn safe_next_rejects_offsite() {
        assert_eq!(safe_next(None), "/ui/");
        assert_eq!(safe_next(Some("https://evil.example/ui/")), "/ui/");
        assert_eq!(safe_next(Some("//evil.example/ui/")), "/ui/");
        assert_eq!(safe_next(Some("/admin")), "/ui/");
        assert_eq!(safe_next(Some("/ui/entity/Alice")), "/ui/entity/Alice");
    }

    #[test]
    fn user_label_strips_token_body() {
        assert_eq!(user_label("alice_deadbeef"), "alice");
        assert_eq!(user_label("shorttok"), "shortt");
    }
}
