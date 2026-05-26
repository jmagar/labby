//! Shared SSH primitives used by deploy and other SSH-backed operators.
//!
//! This module owns:
//!
//! - `SshHostTarget` — one actionable host entry parsed from `~/.ssh/config`.
//! - `parse_ssh_config` — pure parser for the OpenSSH config file.
//!
//! Process-spawning code (`SshSession`, `SshOptions`, `StrictHostKeyChecking`,
//! `shell_quote`) was moved to `crates/lab/src/dispatch/deploy/ssh_session.rs`
//! because it depends on `tokio::process::Command` and belongs in the binary,
//! not in the pure SDK.

use std::collections::HashSet;

/// One actionable SSH host entry parsed from an OpenSSH config file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshHostTarget {
    /// Alias from the `Host` directive.
    pub alias: String,
    /// Resolved hostname metadata from `HostName`, when present.
    pub hostname: Option<String>,
    /// User from the `User` directive, when present.
    pub user: Option<String>,
    /// Port from the `Port` directive, when present.
    pub port: Option<u16>,
    /// `IdentityFile` from the `IdentityFile` directive, when present.
    pub identity_file: Option<String>,
}

/// Parse actionable host entries from an OpenSSH config file.
///
/// Wildcard patterns (`Host *`, `Host media-*`) are skipped — they are
/// templates, not actionable targets. Negated patterns (`!github.com`) are
/// also skipped. `Match` blocks are ignored for V1 of the deploy service;
/// they require runtime expansion and shell access we don't want to run.
#[must_use]
pub fn parse_ssh_config(contents: &str) -> Vec<SshHostTarget> {
    let mut hosts = Vec::new();
    let mut seen = HashSet::new();
    let mut aliases: Vec<String> = Vec::new();
    let mut hostname: Option<String> = None;
    let mut user: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut identity_file: Option<String> = None;
    let mut in_match_block = false;

    let flush = |hosts: &mut Vec<SshHostTarget>,
                 seen: &mut HashSet<String>,
                 aliases: &mut Vec<String>,
                 hostname: &mut Option<String>,
                 user: &mut Option<String>,
                 port: &mut Option<u16>,
                 identity_file: &mut Option<String>| {
        for alias in aliases.drain(..) {
            if seen.insert(alias.clone()) {
                hosts.push(SshHostTarget {
                    alias,
                    hostname: hostname.clone(),
                    user: user.clone(),
                    port: *port,
                    identity_file: identity_file.clone(),
                });
            }
        }
        *hostname = None;
        *user = None;
        *port = None;
        *identity_file = None;
    };

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(keyword) = parts.next() else {
            continue;
        };

        if keyword.eq_ignore_ascii_case("host") {
            flush(
                &mut hosts,
                &mut seen,
                &mut aliases,
                &mut hostname,
                &mut user,
                &mut port,
                &mut identity_file,
            );
            in_match_block = false;
            aliases.extend(
                parts
                    .filter(|alias| {
                        !alias.starts_with('!') && !alias.contains('*') && !alias.contains('?')
                    })
                    .map(ToOwned::to_owned),
            );
            continue;
        }

        if keyword.eq_ignore_ascii_case("match") {
            flush(
                &mut hosts,
                &mut seen,
                &mut aliases,
                &mut hostname,
                &mut user,
                &mut port,
                &mut identity_file,
            );
            in_match_block = true;
            continue;
        }

        if in_match_block {
            continue;
        }

        if keyword.eq_ignore_ascii_case("hostname") {
            hostname = parts.next().map(ToOwned::to_owned);
        } else if keyword.eq_ignore_ascii_case("user") {
            user = parts.next().map(ToOwned::to_owned);
        } else if keyword.eq_ignore_ascii_case("port") {
            port = parts.next().and_then(|p| p.parse::<u16>().ok());
        } else if keyword.eq_ignore_ascii_case("identityfile") {
            identity_file = parts.next().map(ToOwned::to_owned);
        }
    }

    flush(
        &mut hosts,
        &mut seen,
        &mut aliases,
        &mut hostname,
        &mut user,
        &mut port,
        &mut identity_file,
    );
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_alias_hostname_user_port() {
        let raw = "\nHost mini1\n    HostName 10.0.0.11\n    User deploy\n    Port 2222\n";
        let hosts = parse_ssh_config(raw);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "mini1");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.11"));
        assert_eq!(hosts[0].user.as_deref(), Some("deploy"));
        assert_eq!(hosts[0].port, Some(2222));
    }

    #[test]
    fn parses_identity_file_directive() {
        let raw = "Host mini1\n    HostName 10.0.0.11\n    IdentityFile ~/.ssh/id_ed25519\n";
        let hosts = parse_ssh_config(raw);
        assert_eq!(hosts[0].identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
    }

    #[test]
    fn ignores_match_blocks_for_literal_aliases() {
        let raw = "Match user root\n    ForwardAgent no\n";
        assert!(parse_ssh_config(raw).is_empty());
    }

    #[test]
    fn skips_wildcard_and_negated_patterns() {
        let raw = "Host *\n    ForwardAgent yes\n\nHost media-*\n    User root\n\nHost media\n    HostName media.example\n";
        let hosts = parse_ssh_config(raw);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "media");
    }
}
