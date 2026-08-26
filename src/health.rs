// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Jeremy Beker

//! Liveness endpoint and the self-probe used by the Docker HEALTHCHECK.
//!
//! GET /health is unauthenticated and reports only liveness plus the server
//! version — no data-dependent state. The probe (`--health-check`) is a
//! minimal std-only HTTP client so the runtime image needs no curl/wget.

use axum::Router;
use axum::response::Json;
use axum::routing::get;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub fn router() -> Router {
    Router::new().route(
        "/health",
        get(|| async {
            Json(serde_json::json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
            }))
        }),
    )
}

/// Probe http://<host>:<port>/health and report success. Used by
/// `advanced-memory-mcp --health-check` inside the container.
pub fn probe(host: &str, port: u16, timeout: Duration) -> Result<(), String> {
    let addr = (host, port);
    let stream = TcpStream::connect_timeout(
        &std::net::ToSocketAddrs::to_socket_addrs(&addr)
            .map_err(|e| format!("resolve {host}:{port}: {e}"))?
            .next()
            .ok_or_else(|| format!("no address for {host}:{port}"))?,
        timeout,
    )
    .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;

    let mut stream = stream;
    write!(
        stream,
        "GET /health HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("send request: {e}"))?;

    let mut response = String::new();
    stream
        .take(4096)
        .read_to_string(&mut response)
        .map_err(|e| format!("read response: {e}"))?;

    let status_line = response.lines().next().unwrap_or("");
    if status_line.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        Err(format!("unexpected response: {status_line:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_returns_ok_and_version() {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let app = router();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn probe_fails_fast_when_nothing_listens() {
        // Port 1 is essentially never listening locally.
        let err = probe("127.0.0.1", 1, Duration::from_millis(300)).unwrap_err();
        assert!(err.contains("connect"), "got: {err}");
    }
}
