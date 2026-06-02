//! Tests: normalize_user_code shapes, surface/caller destructive gating, oauth subject.
#![cfg(test)]

use super::*;

#[test]
fn normalize_user_code_strips_javascript_markdown_fences() {
    let fenced = "```javascript\nconsole.log('hi');\n```";
    let result = super::normalize_user_code(fenced);

    // PRESENCE: inner code preserved
    assert!(
        result.contains("console.log"),
        "inner code must survive fence stripping"
    );
    // ABSENCE: fences removed
    assert!(
        !result.contains("```"),
        "backtick fences must be stripped, got: {result}"
    );
    assert!(
        !result.contains("javascript"),
        "language tag must be stripped"
    );
}

#[test]
fn normalize_user_code_strips_typescript_fences() {
    let fenced = "```typescript\nconst x: number = 1;\n```";
    let result = super::normalize_user_code(fenced);
    assert!(result.contains("const x: number = 1"));
    assert!(!result.contains("```"));
    assert!(!result.contains("typescript"));
}

#[test]
fn normalize_user_code_wraps_and_calls_bare_async_main() {
    let bare = "async function main() { return 42; }";
    let result = super::normalize_user_code(bare);

    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("async function main()"), "got: {result}");
    assert!(result.contains("return main();"), "got: {result}");
}

#[test]
fn normalize_user_code_wraps_and_calls_bare_sync_main() {
    let bare = "function main() { return 42; }";
    let result = super::normalize_user_code(bare);
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("function main()"), "got: {result}");
    assert!(result.contains("return main();"), "got: {result}");
}

#[test]
fn normalize_user_code_wraps_loose_statement_block() {
    let already_called = "const x = 1;\nmain();";
    let result = super::normalize_user_code(already_called);
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("const x = 1;"));
    assert!(result.contains("return (main())"));
}

#[test]
fn normalize_user_code_returns_trailing_expression() {
    let loose = "const x = await callTool('upstream::github::search_issues', {});\nx.items";
    let result = super::normalize_user_code(loose);
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("const x = await callTool"));
    assert!(result.contains("return (x.items)"), "got: {result}");
}

#[test]
fn normalize_user_code_handles_cloudflare_ast_cases() {
    let assigned_arrow = super::normalize_user_code("const f = () => 1; f()");
    assert!(
        assigned_arrow.starts_with("async () => {"),
        "{assigned_arrow}"
    );
    assert!(assigned_arrow.contains("return (f())"), "{assigned_arrow}");

    let export_arrow = super::normalize_user_code("export default async () => 42");
    assert_eq!(export_arrow, "async () => 42");

    let named = super::normalize_user_code("async function doStuff() { return 42; }");
    assert!(named.contains("return doStuff();"), "{named}");

    let iife = super::normalize_user_code("(async () => 42)()");
    assert!(
        iife.contains("return ((") && iife.contains(")())"),
        "{iife}"
    );
}

#[test]
fn normalize_user_code_wraps_export_default_async_function_as_iife() {
    let exported = "export default async function() { return 42; }";
    let result = super::normalize_user_code(exported);

    assert!(!result.contains("export default"));
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("return (async function"), "got: {result}");
    assert!(result.contains("})();"), "got: {result}");
}

#[test]
fn normalize_user_code_wraps_export_default_sync_function_as_iife() {
    let exported = "export default function() { return 42; }";
    let result = super::normalize_user_code(exported);
    assert!(!result.contains("export default"));
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("return (function"), "got: {result}");
    assert!(result.contains("})();"), "got: {result}");
}

#[test]
fn normalize_user_code_passthrough_for_plain_expressions() {
    let plain = "async () => callTool('lab::test', {})";
    let result = super::normalize_user_code(plain);
    // PRESENCE: no transformation applied
    assert_eq!(
        result, plain,
        "async arrow expressions must pass through unchanged"
    );
}

#[test]
fn normalize_user_code_strips_arrow_expression_trailing_semicolon() {
    let result = super::normalize_user_code("async () => 42;");
    assert!(result.starts_with("async () =>"), "got: {result}");
    assert!(
        !result.trim_end().ends_with(';'),
        "normalized arrow must not keep the source trailing semicolon: {result}"
    );
}

// ── CodeModeSurface allow_destructive_actions ─────────────────────────────

#[test]
fn code_mode_surface_mcp_gates_on_flag() {
    let mcp_allow = super::CodeModeSurface::Mcp {
        allow_destructive_actions: true,
    };
    let mcp_deny = super::CodeModeSurface::Mcp {
        allow_destructive_actions: false,
    };

    // PRESENCE: true flag → allowed
    assert!(
        mcp_allow.allow_destructive_actions(),
        "Mcp with allow_destructive_actions=true must return true"
    );
    // PRESENCE: false flag → denied
    assert!(
        !mcp_deny.allow_destructive_actions(),
        "Mcp with allow_destructive_actions=false must return false"
    );
}

#[test]
fn code_mode_surface_cli_always_allows_destructive() {
    let cli = super::CodeModeSurface::Cli;
    // PRESENCE: CLI always permits
    assert!(
        cli.allow_destructive_actions(),
        "CLI surface must always allow destructive actions"
    );
}

// ── destructive_permitted: surface confirmation gate ─────────────────────

fn scoped(scopes: &[&str]) -> super::CodeModeCaller {
    super::CodeModeCaller::Scoped {
        scopes: scopes.iter().map(ToString::to_string).collect(),
        sub: None,
    }
}

#[test]
fn destructive_denied_for_execute_scope_on_mcp_deny_surface() {
    let surface = super::CodeModeSurface::Mcp {
        allow_destructive_actions: false,
    };
    assert!(
        !super::destructive_permitted(surface, &scoped(&["lab:admin"])),
        "lab:admin caller still needs explicit confirm for destructive MCP Code Mode calls"
    );
    assert!(
        !super::destructive_permitted(surface, &scoped(&["lab"])),
        "lab caller still needs explicit confirm for destructive MCP Code Mode calls"
    );
}

#[test]
fn destructive_denied_for_read_scope_on_mcp_deny_surface() {
    // PRESENCE: a read-only caller without confirm stays denied.
    let surface = super::CodeModeSurface::Mcp {
        allow_destructive_actions: false,
    };
    assert!(
        !super::destructive_permitted(surface, &scoped(&["lab:read"])),
        "lab:read caller must NOT be permitted destructive actions without confirm"
    );
}

#[test]
fn destructive_permitted_via_surface_flag_regardless_of_scope() {
    // PRESENCE: confirm:true (or CLI) permits even a read-only caller —
    // the surface flag is an independent allow path.
    let mcp_allow = super::CodeModeSurface::Mcp {
        allow_destructive_actions: true,
    };
    assert!(
        super::destructive_permitted(mcp_allow, &scoped(&["lab:read"])),
        "confirm:true surface must permit destructive actions for any caller"
    );
    assert!(
        super::destructive_permitted(super::CodeModeSurface::Cli, &scoped(&["lab:read"])),
        "CLI surface must permit destructive actions for any caller"
    );
    assert!(
        !super::destructive_permitted(
            super::CodeModeSurface::Mcp {
                allow_destructive_actions: false
            },
            &super::CodeModeCaller::TrustedLocal,
        ),
        "TrustedLocal MCP caller must still provide destructive confirmation"
    );
}

// ── CodeModeCaller oauth_subject ──────────────────────────────────────────

#[test]
fn oauth_subject_uses_sub_for_non_admin_caller() {
    // A non-admin caller with its own sub authenticates as itself so a
    // personal upstream grant is used.
    let caller = super::CodeModeCaller::Scoped {
        scopes: vec!["lab".to_string()],
        sub: Some("user@example.com".to_string()),
    };

    assert_eq!(
        caller.oauth_subject(),
        Some("user@example.com"),
        "non-admin oauth_subject must return the caller's JWT sub"
    );
}

#[test]
fn oauth_subject_collapses_admin_to_shared_gateway_subject() {
    // Regression (lab-om1ou): admin callers must collapse to the shared
    // gateway subject — parity with `oauth_upstream_subject_for_request` —
    // so they reuse the gateway-owned upstream credential and the proactive
    // refresh path is reached. Otherwise OAuth upstreams (axon) get stranded.
    let caller = super::CodeModeCaller::Scoped {
        scopes: vec!["lab".to_string(), "lab:admin".to_string()],
        sub: Some("115693937070075916387".to_string()),
    };

    assert_eq!(
        caller.oauth_subject(),
        Some(super::SHARED_GATEWAY_OAUTH_SUBJECT),
        "lab:admin callers must collapse to the shared gateway subject, not their raw sub"
    );
}

#[test]
fn oauth_subject_falls_back_to_shared_when_sub_absent() {
    let caller = super::CodeModeCaller::Scoped {
        scopes: vec!["lab:admin".to_string()],
        sub: None,
    };

    // PRESENCE: falls back to some non-None shared subject
    let subject = caller.oauth_subject();
    assert!(
        subject.is_some(),
        "oauth_subject must return Some (shared fallback) when sub is None"
    );
    // ABSENCE: not the same as the user-specific email
    assert_ne!(
        subject,
        Some("user@example.com"),
        "fallback subject must not be user-specific"
    );
}

#[test]
fn oauth_subject_trusted_local_returns_shared_subject() {
    let caller = super::CodeModeCaller::TrustedLocal;
    // PRESENCE: trusted local also returns a subject (the shared gateway subject)
    assert!(
        caller.oauth_subject().is_some(),
        "TrustedLocal must return Some oauth_subject"
    );
}

// ── CodeModeCaller can_execute / can_read scope checks ────────────────────

#[test]
fn scoped_caller_can_execute_with_lab_scope() {
    let caller = super::CodeModeCaller::Scoped {
        scopes: vec!["lab".to_string()],
        sub: None,
    };
    assert!(caller.can_execute());
    assert!(caller.can_read());
}

#[test]
fn scoped_caller_read_only_cannot_execute() {
    let caller = super::CodeModeCaller::Scoped {
        scopes: vec!["lab:read".to_string()],
        sub: None,
    };
    // PRESENCE: can read
    assert!(caller.can_read());
    // ABSENCE: cannot execute
    assert!(
        !caller.can_execute(),
        "lab:read scope must not permit execution"
    );
}

// ── token_estimate_divisor affects truncation (#12b) ─────────────────────
