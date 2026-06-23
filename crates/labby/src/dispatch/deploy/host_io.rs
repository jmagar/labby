//! `HostIo` trait and the production `SshHostIo` implementation.
//!
//! The trait abstracts all remote I/O so stage functions can be tested without
//! a real SSH server. Production code uses `SshHostIo`; tests substitute a
//! recording fake (`test_support::RecordingIo` in `runner.rs`).

use std::future::Future;
use std::pin::Pin;

use super::ssh_session::{SshHostTarget, SshSession};
use labby_apis::deploy::DeployError;

/// Low-level primitive the runner uses to talk to a single host.
///
/// The production implementation is `SshHostIo` backed by `SshSession`.
/// Tests substitute a recording fake that captures the op stream and
/// returns scripted responses without touching the network.
///
/// All methods are sync fns returning `'static` futures. This avoids
/// higher-ranked trait bound (HRTB) errors in `Box::pin(async move { … } +
/// Send + 'static)` contexts (Rust issue #100013). Implementations must do
/// all `&self` work synchronously and capture only owned values in the
/// returned future.
pub trait HostIo: Send + Sync {
    fn run_argv(
        &self,
        argv: &[&str],
    ) -> Pin<Box<dyn Future<Output = Result<(i32, String, String), DeployError>> + Send + 'static>>;

    fn upload_stream<R>(
        &self,
        remote_path: &str,
        reader: R,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DeployError>> + Send + 'static>>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static;

    fn sha256_remote(
        &self,
        remote_path: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, DeployError>> + Send + 'static>>;
}

/// Production `HostIo` impl backed by `labby_apis::core::ssh::SshSession`.
///
/// `host` is carried alongside the session so errors can be tagged with the
/// alias the caller asked for, not the underlying hostname.
pub struct SshHostIo {
    pub host: String,
    pub session: SshSession,
}

impl SshHostIo {
    #[must_use]
    pub fn new(host: impl Into<String>, target: SshHostTarget) -> Self {
        Self {
            host: host.into(),
            session: SshSession::new(target),
        }
    }
}

impl HostIo for SshHostIo {
    fn run_argv(
        &self,
        argv: &[&str],
    ) -> Pin<Box<dyn Future<Output = Result<(i32, String, String), DeployError>> + Send + 'static>>
    {
        let fut = self.session.run_command(argv);
        let host = self.host.clone();
        Box::pin(async move {
            fut.await.map_err(|e| DeployError::SshUnreachable {
                host: format!("{host}: {e}"),
            })
        })
    }

    fn upload_stream<R>(
        &self,
        remote_path: &str,
        reader: R,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DeployError>> + Send + 'static>>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let fut = self.session.upload_stream(remote_path, reader);
        let host = self.host.clone();
        Box::pin(async move {
            fut.await.map_err(|e| DeployError::TransferFailed {
                host,
                reason: e.to_string(),
            })
        })
    }

    fn sha256_remote(
        &self,
        remote_path: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, DeployError>> + Send + 'static>> {
        let fut = self.session.sha256_remote(remote_path);
        let host = self.host.clone();
        Box::pin(async move {
            fut.await.map_err(|e| DeployError::SshUnreachable {
                host: format!("{host}: {e}"),
            })
        })
    }
}
