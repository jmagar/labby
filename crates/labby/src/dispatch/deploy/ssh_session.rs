//! SSH process-spawning primitives used exclusively by the `deploy` service.
//!
//! This module contains types that spawn `tokio::process::Command` processes
//! and therefore belong in the `lab` binary, not in the `lab-apis` SDK.
//!
//! **Shared pure types** (`SshHostTarget`, `parse_ssh_config`) remain in
//! `labby_apis::core::ssh`. Only the process-spawning code lives here.

use std::future::Future;
use std::pin::Pin;

pub use labby_apis::core::ssh::SshHostTarget;

/// Hardened options for outbound `ssh` command invocations.
///
/// `SshOptions::hardened()` is the default used by the `deploy` service. It
/// opts into ControlMaster/ControlPersist for session reuse, enables strict
/// host key checking, and disables agent forwarding.
#[derive(Debug, Clone)]
pub struct SshOptions {
    pub connect_timeout: std::time::Duration,
    pub server_alive_interval: std::time::Duration,
    pub server_alive_count_max: u32,
    pub forward_agent: bool,
    pub strict_host_key_checking: StrictHostKeyChecking,
    pub control_persist: Option<std::time::Duration>,
    pub control_path_template: Option<String>,
}

/// Host-key checking mode for `-oStrictHostKeyChecking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictHostKeyChecking {
    Yes,
    // Not yet constructed: available for callers that need reduced host-key strictness.
    #[allow(dead_code)]
    AcceptNew,
    // Not yet constructed: available for callers that need reduced host-key strictness.
    #[allow(dead_code)]
    No,
}

impl SshOptions {
    #[must_use]
    pub fn hardened() -> Self {
        Self {
            connect_timeout: std::time::Duration::from_secs(10),
            server_alive_interval: std::time::Duration::from_secs(15),
            server_alive_count_max: 3,
            forward_agent: false,
            strict_host_key_checking: StrictHostKeyChecking::Yes,
            control_persist: Some(std::time::Duration::from_secs(60)),
            control_path_template: Some("~/.lab/ssh/cm-%r@%h:%p".to_string()),
        }
    }

    /// Render this option set into the `-o` flags the `ssh` binary expects.
    #[must_use]
    pub fn to_openssh_args(&self) -> Vec<String> {
        let mut a = vec![
            format!("-oConnectTimeout={}", self.connect_timeout.as_secs()),
            format!(
                "-oServerAliveInterval={}",
                self.server_alive_interval.as_secs()
            ),
            format!("-oServerAliveCountMax={}", self.server_alive_count_max),
            format!(
                "-oForwardAgent={}",
                if self.forward_agent { "yes" } else { "no" }
            ),
            format!(
                "-oStrictHostKeyChecking={}",
                match self.strict_host_key_checking {
                    StrictHostKeyChecking::Yes => "yes",
                    StrictHostKeyChecking::AcceptNew => "accept-new",
                    StrictHostKeyChecking::No => "no",
                }
            ),
        ];
        if let (Some(persist), Some(path)) = (self.control_persist, &self.control_path_template) {
            a.push("-oControlMaster=auto".into());
            a.push(format!("-oControlPersist={}s", persist.as_secs()));
            a.push(format!("-oControlPath={path}"));
        }
        a
    }
}

/// Errors produced by `SshSession` operations.
///
/// These are deliberately coarse — the dispatch layer maps them into
/// `DeployError` variants with host context, so `SshError` only needs to
/// preserve the transport-level failure shape.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("ssh spawn failed: {0}")]
    Spawn(String),
    #[error("ssh io: {0}")]
    Io(String),
}

/// Quote a string for embedding in a POSIX `sh` single-quoted context.
///
/// Wraps `s` in single quotes and replaces any embedded `'` with `'\''` so
/// the result is safe for `sh -c '…'` command construction.  This is the only
/// quoting helper permitted in this module; all remote path and argv
/// interpolation must go through it.
///
/// # Examples
/// ```ignore
/// assert_eq!(shell_quote("foo"),          "'foo'");
/// assert_eq!(shell_quote("foo bar"),      "'foo bar'");
/// assert_eq!(shell_quote("foo's"),        "'foo'\\''s'");
/// assert_eq!(shell_quote("foo;rm -rf /"), "'foo;rm -rf /'");
/// ```
pub fn shell_quote(s: &str) -> String {
    // Replace every ' with '\'' (end-quote, literal-quote, re-open-quote).
    let escaped = s.replace('\'', r"'\''");
    format!("'{escaped}'")
}

/// A reusable `ssh` invocation target backed by `tokio::process::Command`.
///
/// `SshSession` does not maintain a long-lived connection itself — each
/// `run_command`, `upload_stream`, and `sha256_remote` spawns a fresh `ssh`
/// process. `SshOptions::hardened()` enables ControlMaster/ControlPersist so
/// the second+ connection to a given host reuses the kernel-side control
/// socket, avoiding the handshake cost.
///
/// **Remote shell behaviour.** OpenSSH concatenates all trailing argv tokens
/// with spaces and hands the result to the remote user's default shell.
/// `run_command` therefore shell-quotes every element via [`shell_quote`]
/// before passing it, making separate-arg safety an illusion that must be
/// actively enforced at this layer. A single documented exception exists in
/// `upload_stream`: remote redirect must go through `sh -c "cat > '...'"`
/// because OpenSSH does not expose a redirect primitive; the path is
/// shell-quoted and allowlist-validated by the caller (see
/// `crates/lab/src/dispatch/deploy/params.rs::validate_remote_path`).
#[derive(Debug, Clone)]
pub struct SshSession {
    pub target: SshHostTarget,
    pub options: SshOptions,
}

impl SshSession {
    /// Construct a session pointed at `target` with hardened defaults.
    #[must_use]
    pub fn new(target: SshHostTarget) -> Self {
        Self {
            target,
            options: SshOptions::hardened(),
        }
    }

    // Not yet called outside tests; kept for callers that need non-hardened options.
    #[allow(dead_code)]
    #[must_use]
    pub const fn with_options(target: SshHostTarget, options: SshOptions) -> Self {
        Self { target, options }
    }

    /// Render the `[user@]hostname` connect target.
    fn connect_target(&self) -> String {
        let host = self
            .target
            .hostname
            .as_deref()
            .unwrap_or(self.target.alias.as_str());
        if let Some(user) = self.target.user.as_deref() {
            format!("{user}@{host}")
        } else {
            host.to_string()
        }
    }

    /// Collect `-o…` flags + `-p <port>` + `-i <identity>` from `options`
    /// and `target`.
    fn base_args(&self) -> Vec<String> {
        let mut args = self.options.to_openssh_args();
        if let Some(port) = self.target.port {
            args.push("-p".into());
            args.push(port.to_string());
        }
        if let Some(id) = self.target.identity_file.as_deref() {
            args.push("-i".into());
            args.push(id.to_string());
        }
        // Reject any attempt to pass a BatchMode-disabling option through
        // target fields. `SshOptions::hardened()` keeps interactive prompts
        // off via ControlPersist; callers need no password tty.
        args.push("-oBatchMode=yes".into());
        args
    }

    /// Run `argv` on the remote host.
    ///
    /// OpenSSH concatenates all trailing tokens with spaces and hands the
    /// resulting string to the remote shell, so every element of `argv` is
    /// shell-quoted via [`shell_quote`] before it reaches the network. This
    /// prevents command injection when paths or arguments contain spaces,
    /// single quotes, semicolons, or other shell metacharacters.
    ///
    /// Returns `(exit_code, stdout, stderr)`. A nonzero exit code is not an
    /// `Err` — the caller is expected to decide whether the nonzero exit is
    /// a failure.
    ///
    /// Returns a `'static` future by doing all `&self` work synchronously
    /// (building the `Command`) before returning. This avoids higher-ranked
    /// trait bound (HRTB) errors in `Box::pin(async move { ... } + Send +
    /// 'static)` contexts (Rust issue #100013).
    pub fn run_command(
        &self,
        argv: &[&str],
    ) -> Pin<Box<dyn Future<Output = Result<(i32, String, String), SshError>> + Send + 'static>>
    {
        let mut cmd = tokio::process::Command::new("ssh");
        for a in self.base_args() {
            cmd.arg(a);
        }
        cmd.arg(self.connect_target());
        for a in argv {
            cmd.arg(shell_quote(a));
        }
        Box::pin(async move {
            let output = cmd
                .output()
                .await
                .map_err(|e| SshError::Spawn(e.to_string()))?;
            let code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Ok((code, stdout, stderr))
        })
    }

    /// Stream `reader` into a file at `remote_path` on the target.
    ///
    /// **Shell exception.** This is the only `SshSession` path that emits a
    /// `sh -c` subshell — OpenSSH has no native "write stdin to file"
    /// primitive, so we invoke `sh -c "cat > '<remote_path>'"` on the remote
    /// side. `remote_path` must be allowlist-validated by the caller
    /// (`deploy`'s `validate_remote_path` only permits `/usr/local/bin/` and
    /// `/opt/lab/bin/` prefixes, and rejects `..`), which makes the single
    /// quoting safe.
    ///
    /// Returns the number of bytes copied into the child's stdin.
    ///
    /// Returns a `'static` future by doing all `&self` work synchronously
    /// (building the `Command`) before returning. `R` must be `'static` so
    /// the captured reader can live in the returned future.
    pub fn upload_stream<R>(
        &self,
        remote_path: &str,
        mut reader: R,
    ) -> Pin<Box<dyn Future<Output = Result<u64, SshError>> + Send + 'static>>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        use std::process::Stdio;

        // Shell-quote the path so embedded single quotes do not break the
        // `sh -c` context.  The path is also allowlist-validated upstream
        // (`validate_remote_path`), providing defence in depth.
        let quoted = shell_quote(remote_path);
        let redirect = format!("cat > {quoted}");
        let mut cmd = tokio::process::Command::new("ssh");
        for a in self.base_args() {
            cmd.arg(a);
        }
        cmd.arg(self.connect_target());
        cmd.arg("sh").arg("-c").arg(&redirect);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        Box::pin(async move {
            use tokio::io::AsyncWriteExt;

            let mut child = cmd.spawn().map_err(|e| SshError::Spawn(e.to_string()))?;

            let mut stdin = match child.stdin.take() {
                Some(s) => s,
                None => {
                    child.kill().await.ok();
                    child.wait().await.ok();
                    return Err(SshError::Io("ssh stdin missing".into()));
                }
            };
            let bytes = match tokio::io::copy(&mut reader, &mut stdin).await {
                Ok(n) => n,
                Err(e) => {
                    child.kill().await.ok();
                    child.wait().await.ok();
                    return Err(SshError::Io(e.to_string()));
                }
            };
            if let Err(e) = stdin.shutdown().await {
                child.kill().await.ok();
                child.wait().await.ok();
                return Err(SshError::Io(e.to_string()));
            }
            drop(stdin);
            let out = child
                .wait_with_output()
                .await
                .map_err(|e| SshError::Io(e.to_string()))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(SshError::Io(format!(
                    "remote cat failed ({}): {}",
                    out.status.code().unwrap_or(-1),
                    stderr.trim()
                )));
            }
            Ok(bytes)
        })
    }

    /// Probe `sha256sum <path>` on the remote host.
    ///
    /// Returns `Ok(Some(hex))` when the probe succeeds, `Ok(None)` when the
    /// file is absent or the tool exits nonzero, and `Err` only when the
    /// `ssh` spawn itself fails.
    pub fn sha256_remote(
        &self,
        remote_path: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, SshError>> + Send + 'static>> {
        let fut = self.run_command(&["sha256sum", remote_path]);
        Box::pin(async move {
            let (code, stdout, _stderr) = fut.await?;
            if code != 0 {
                return Ok(None);
            }
            // sha256sum output: "<hex>  <path>\n"
            let hex = stdout.split_whitespace().next().map(str::to_string);
            // Must be 64 lowercase hex chars; reject anything else.
            let ok = hex
                .as_ref()
                .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
            Ok(if ok { hex } else { None })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_plain() {
        assert_eq!(shell_quote("foo"), "'foo'");
    }

    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(shell_quote("foo bar"), "'foo bar'");
    }

    #[test]
    fn shell_quote_with_single_quote() {
        assert_eq!(shell_quote("foo's"), "'foo'\\''s'");
    }

    #[test]
    fn shell_quote_injection_attempt() {
        assert_eq!(shell_quote("foo;rm -rf /"), "'foo;rm -rf /'");
    }

    #[test]
    fn shell_quote_path_with_embedded_quote() {
        // e.g. a path like /opt/it's-fine/bin
        assert_eq!(
            shell_quote("/opt/it's-fine/bin"),
            "'/opt/it'\\''s-fine/bin'"
        );
    }

    #[test]
    fn session_options_include_control_master_and_hardening_defaults() {
        let opts = SshOptions::hardened();
        assert_eq!(opts.connect_timeout.as_secs(), 10);
        assert_eq!(opts.server_alive_interval.as_secs(), 15);
        assert_eq!(opts.server_alive_count_max, 3);
        assert!(!opts.forward_agent);
        assert_eq!(opts.strict_host_key_checking, StrictHostKeyChecking::Yes);
        assert!(opts.control_persist.is_some());
    }

    #[test]
    fn openssh_args_contain_hardening_flags() {
        let args = SshOptions::hardened().to_openssh_args();
        assert!(args.iter().any(|a| a == "-oForwardAgent=no"));
        assert!(args.iter().any(|a| a == "-oStrictHostKeyChecking=yes"));
        assert!(args.iter().any(|a| a == "-oControlMaster=auto"));
    }

    #[test]
    fn session_connect_target_uses_user_and_hostname_when_present() {
        let t = SshHostTarget {
            alias: "mini1".into(),
            hostname: Some("10.0.0.11".into()),
            user: Some("deploy".into()),
            port: None,
            identity_file: None,
        };
        let s = SshSession::new(t);
        assert_eq!(s.connect_target(), "deploy@10.0.0.11");
    }

    #[test]
    fn session_connect_target_falls_back_to_alias_without_hostname() {
        let t = SshHostTarget {
            alias: "mini1".into(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
        };
        let s = SshSession::new(t);
        assert_eq!(s.connect_target(), "mini1");
    }

    #[test]
    fn session_base_args_include_port_and_identity_and_batchmode() {
        let t = SshHostTarget {
            alias: "mini1".into(),
            hostname: Some("10.0.0.11".into()),
            user: Some("deploy".into()),
            port: Some(2222),
            identity_file: Some("~/.ssh/id_ed25519".into()),
        };
        let s = SshSession::new(t);
        let args = s.base_args();
        // port appears as two tokens so ssh parses it as `-p <n>`
        let idx = args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(args[idx + 1], "2222");
        let ii = args.iter().position(|a| a == "-i").expect("-i present");
        assert_eq!(args[ii + 1], "~/.ssh/id_ed25519");
        assert!(args.iter().any(|a| a == "-oBatchMode=yes"));
    }
}
