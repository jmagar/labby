//! Minimal loopback HTTP health server for node-mode processes.
//!
//! Binds to 127.0.0.1 only. Routes: GET /health, GET /ready.
//! Implemented as raw tokio TCP to keep the node-runtime feature
//! free of axum/tower/tower-http dependencies.

use std::process::ExitCode;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::Duration;

/// Bind a loopback-only HTTP health server on `127.0.0.1:{port}` and run
/// forever, accepting connections. Returns `Ok(ExitCode::SUCCESS)` only if
/// the loop exits cleanly (which it never does in normal operation — this is
/// the process keep-alive path for node-mode processes).
pub async fn run_loopback_health_server(port: u16) -> Result<ExitCode> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind loopback health server to {addr}"))?;

    tracing::info!(
        surface = "node",
        service = "health",
        action = "server.start",
        bind_addr = %addr,
        "node loopback health server listening"
    );

    loop {
        match listener.accept().await {
            Ok((mut stream, _peer)) => {
                tokio::spawn(async move {
                    handle_health_connection(&mut stream).await;
                });
            }
            Err(error) => {
                tracing::warn!(
                    surface = "node",
                    service = "health",
                    action = "server.accept_error",
                    error = %error,
                    "loopback health server accept error"
                );
            }
        }
    }
}

async fn handle_health_connection(stream: &mut tokio::net::TcpStream) {
    let mut buf = [0u8; 512];
    let n = match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => n,
        _ => return,
    };
    let request = std::str::from_utf8(&buf[..n]).unwrap_or("");
    let first_line = request.lines().next().unwrap_or("");

    tracing::debug!(
        surface = "node",
        service = "health",
        action = "request.recv",
        path = %first_line,
        "health request received"
    );

    let (status, body) = if first_line.starts_with("GET /health") {
        ("200 OK", r#"{"ok":true}"#)
    } else if first_line.starts_with("GET /ready") {
        ("200 OK", r#"{"ready":true}"#)
    } else {
        ("404 Not Found", r#"{"error":"not found"}"#)
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    drop(stream.write_all(response.as_bytes()).await);

    tracing::debug!(
        surface = "node",
        service = "health",
        action = "response.sent",
        status = %status,
        "health response sent"
    );
}
