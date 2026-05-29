//! Runtime JS proxy generation for Code Mode.
//!
//! Generates the executable `var codemode = {...}` proxy object that is sent
//! through the Code Mode Start protocol and injected into the sandbox after
//! `callTool` is defined. The proxy lets the agent call
//! `codemode.<upstream>.<tool>(params)`, which routes to
//! `callTool("upstream::<upstream>::<dotted.name>", params)`.
//!
//! This is RUNTIME JS, not a TypeScript declaration: it never enters the
//! model's context (unlike the deleted typed-preamble-in-tool-description
//! machinery from commit 780c67d3). Surfacing types/schemas via `search` is a
//! separate follow-up; this module only restores the executable proxy.

use crate::dispatch::upstream::types::UpstreamTool;

// ────────────────────────────────────────────────────────────────────────────
// Tool name conversion (snake_case — Cloudflare Code Mode parity)
// ────────────────────────────────────────────────────────────────────────────
//
// Cloudflare's Code Mode normalizes tool ids like `my-server.list-items` to
// `my_server_list_items` (all separators → `_`). We do the same so that an
// LLM trained on Cloudflare examples calls the right names.

/// JavaScript reserved words that need an underscore suffix.
const JS_RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Convert a dotted/hyphenated/slashed/coloned tool name to snake_case.
///
/// Examples (Cloudflare parity):
/// - `movie.search` → `movie_search`
/// - `tv-show.get` → `tv_show_get`
/// - `create/issue` → `create_issue`
/// - `list:repos` → `list_repos`
/// - `delete` → `delete_` (reserved word)
/// - `2fa_setup` → `_2fa_setup` (leading digit prefixed with `_`)
///
/// KNOWN COLLISION: `movie.search` and `movie_search` both map to `movie_search`
/// — last insert wins when building the namespace. A `tracing::debug!` is emitted
/// when a collision is detected.
pub fn tool_name_to_snake(name: &str) -> String {
    // Split on dots, hyphens, forward-slashes, and colons; rejoin with `_`.
    // Underscores already in segments are preserved.
    let snake: String = name
        .split(['.', '-', '/', ':'])
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    // Prefix with `_` if the result starts with a digit (invalid JS identifier start).
    let snake = if snake.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{snake}")
    } else {
        snake
    };

    if JS_RESERVED.contains(&snake.as_str()) {
        format!("{snake}_")
    } else {
        snake
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JS proxy generation (runtime executable, not type declarations)
// ────────────────────────────────────────────────────────────────────────────

/// Generate a JavaScript preamble string that defines the `codemode` proxy
/// namespace, plus `codemode.__meta__.upstreams()` and a `__upstreams__`
/// script-global, for use inside the sandbox.
///
/// The output is a JS snippet (not TypeScript) that is prepended to (Boa) or
/// injected into (Javy) the user code before being sent to the runner
/// subprocess. It relies on `callTool` already being registered in the sandbox.
///
/// The output ends with `var` declarations (no completion value), so when it is
/// concatenated in front of a trailing IIFE the IIFE's promise remains the
/// `eval` completion value.
///
/// `tools` — the upstream tools to expose
/// `upstreams` — sorted, deduplicated list of upstream names
pub fn generate_js_proxy(tools: &[UpstreamTool], upstreams: &[String]) -> String {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    // Group tools by upstream name, sorted for deterministic output.
    let mut by_upstream: BTreeMap<&str, Vec<&UpstreamTool>> = BTreeMap::new();
    for tool in tools {
        by_upstream
            .entry(tool.upstream_name.as_ref())
            .or_default()
            .push(tool);
    }

    let mut parts = String::new();

    // Accumulate method definitions keyed by the snake_cased UPSTREAM name so the
    // namespace is reachable via dot notation (`codemode.arcane_mcp.tool(...)`),
    // not just bracket access. Hyphenated upstreams (arcane-mcp, github-chat, …)
    // would otherwise be unreachable as `codemode.arcane-mcp` (parses as
    // subtraction). The `callTool` id keeps the RAW upstream name. Two raw
    // upstreams that snake-collide merge into one namespace object (tools are not
    // dropped); a per-tool snake collision is last-wins inside the object literal.
    let mut by_snake_upstream: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (upstream_name, upstream_tools) in &by_upstream {
        let upstream_snake = tool_name_to_snake(upstream_name);

        // Build snake_case → dotted name mapping, last registration wins on collision.
        let mut snake_to_dotted: BTreeMap<String, String> = BTreeMap::new();
        let mut sorted_tools = upstream_tools.to_vec();
        sorted_tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));
        for tool in &sorted_tools {
            let snake = tool_name_to_snake(tool.tool.name.as_ref());
            if snake_to_dotted.contains_key(&snake) {
                tracing::debug!(
                    upstream = *upstream_name,
                    tool_name = tool.tool.name.as_ref(),
                    snake_name = %snake,
                    "Code Mode proxy: tool name collision detected, last registration wins"
                );
            }
            snake_to_dotted.insert(snake, tool.tool.name.to_string());
        }

        let method_defs = by_snake_upstream.entry(upstream_snake).or_default();
        for (snake, dotted) in &snake_to_dotted {
            // callTool id uses the RAW upstream + RAW tool name.
            let tool_id = format!("upstream::{upstream_name}::{dotted}");
            let tool_id_json =
                serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"unknown\"".to_string());
            // Always use a JSON-quoted property key so that any residual special
            // characters in the snake_case name (e.g. from exotic tool schemas) never
            // cause a JS syntax error inside QuickJS/Boa.
            let snake_json =
                serde_json::to_string(snake.as_str()).unwrap_or_else(|_| format!("\"{snake}\""));
            // Use `p == null ? {} : p` (not `?? {}`) so the proxy does not depend
            // on the engine supporting the nullish-coalescing operator.
            method_defs.push(format!(
                "    {snake_json}: function(p) {{ return callTool({tool_id_json}, p == null ? {{}} : p); }}"
            ));
        }
    }

    for (upstream_snake, method_defs) in &by_snake_upstream {
        let upstream_snake_json =
            serde_json::to_string(upstream_snake).unwrap_or_else(|_| "\"unknown\"".to_string());
        let methods = method_defs.join(",\n");
        let _ = write!(
            parts,
            "codemode[{upstream_snake_json}] = {{\n{methods}\n}};\n"
        );
    }

    // Emit __meta__.upstreams value.
    let upstreams_json = serde_json::to_string(upstreams).unwrap_or_else(|_| "[]".to_string());

    format!(
        "// Code Mode proxy — auto-generated\n\
         var codemode = {{}};\n\
         {parts}\
         codemode.__meta__ = {{ upstreams: function() {{ return Promise.resolve({upstreams_json}); }} }};\n\
         var __upstreams__ = {upstreams_json};\n"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::Tool;
    use serde_json::Value;

    use super::*;
    use crate::dispatch::upstream::types::UpstreamTool;

    // ── tool_name_to_snake ────────────────────────────────────────────────────

    #[test]
    fn tool_name_to_snake_converts_dotted_names() {
        // PRESENCE: basic dotted/hyphenated name conversion (Cloudflare parity)
        assert_eq!(tool_name_to_snake("movie.search"), "movie_search");
        assert_eq!(tool_name_to_snake("tv-show.get"), "tv_show_get");
        // PRESENCE: reserved word gets underscore suffix
        assert_eq!(tool_name_to_snake("delete"), "delete_");
        // ABSENCE: separators must not appear in snake output
        assert!(!tool_name_to_snake("movie.search").contains('.'));
        assert!(!tool_name_to_snake("tv-show.get").contains('-'));
    }

    #[test]
    fn tool_name_to_snake_handles_slashes_and_colons() {
        // PRESENCE: forward-slashes and colons join with underscore
        assert_eq!(tool_name_to_snake("create/issue"), "create_issue");
        assert_eq!(tool_name_to_snake("list:repos"), "list_repos");
        assert_eq!(
            tool_name_to_snake("repos/create/branch"),
            "repos_create_branch"
        );
        // PRESENCE: already-snake input is preserved
        assert_eq!(tool_name_to_snake("create_issue"), "create_issue");
        // PRESENCE: leading digit gets prefixed with underscore
        assert_eq!(tool_name_to_snake("2fa_setup"), "_2fa_setup");
        // ABSENCE: separators must not appear in output (would break JS syntax)
        assert!(!tool_name_to_snake("create/issue").contains('/'));
        assert!(!tool_name_to_snake("list:repos").contains(':'));
    }

    // ── generate_js_proxy ─────────────────────────────────────────────────────

    #[test]
    fn generate_js_proxy_quoted_keys_tolerate_special_chars() {
        // Tool with a slash in the name — previously caused "Exception generated
        // by QuickJS" because the unquoted key was a JS syntax error.
        let schema = Arc::new(
            serde_json::from_value::<serde_json::Map<String, Value>>(
                serde_json::json!({"type": "object", "properties": {}}),
            )
            .unwrap(),
        );
        let tool = UpstreamTool {
            upstream_name: Arc::from("github"),
            tool: Tool::new("create/issue", "Create an issue", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["github".to_string()]);

        // PRESENCE: the upstream object must be present
        assert!(
            js.contains("codemode[\"github\"]"),
            "upstream block missing"
        );
        // PRESENCE: the tool key must appear as a quoted string (not unquoted)
        assert!(
            js.contains("\"create_issue\""),
            "snake_case key must be quoted"
        );
        // ABSENCE: no unquoted slash-containing key that would break JS syntax
        assert!(
            !js.contains("create/issue:"),
            "slash in unquoted key would break JS"
        );
        // PRESENCE: callTool must be wired to the original dotted tool id
        assert!(
            js.contains("upstream::github::create/issue"),
            "original tool id must be preserved"
        );
    }

    #[test]
    fn generate_js_proxy_emits_codemode_global_and_method() {
        let schema = Arc::new(serde_json::Map::new());
        let tool = UpstreamTool {
            upstream_name: Arc::from("radarr"),
            tool: Tool::new("movie.search", "Search for movies", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["radarr".to_string()]);

        // PRESENCE: declares the script-global `var codemode`
        assert!(js.contains("var codemode = {}"), "must declare codemode");
        // PRESENCE: snake_case method routes to the dotted upstream id
        assert!(
            js.contains("upstream::radarr::movie.search"),
            "method must route to dotted tool id"
        );
        // PRESENCE: __meta__.upstreams reflects the upstream list
        assert!(
            js.contains("[\"radarr\"]"),
            "upstreams list must be embedded"
        );
        // PRESENCE: null-safe params guard (no nullish-coalescing dependency)
        assert!(
            js.contains("p == null ? {} : p"),
            "must use null-safe params guard"
        );
        // ABSENCE: must not use nullish-coalescing (engine-portability)
        assert!(!js.contains("?? {}"), "must not depend on ?? operator");
    }

    #[test]
    fn generate_js_proxy_snake_cases_hyphenated_upstream_keys() {
        // Hyphenated upstreams (arcane-mcp, github-chat, …) must be reachable via
        // dot notation `codemode.arcane_mcp.tool(...)`, not just bracket access —
        // `codemode.arcane-mcp` parses as subtraction. The callTool id must keep
        // the RAW upstream name so the gateway routes to the real server.
        let schema = Arc::new(serde_json::Map::new());
        let tool = UpstreamTool {
            upstream_name: Arc::from("arcane-mcp"),
            tool: Tool::new("arcane", "Docker management", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["arcane-mcp".to_string()]);

        // PRESENCE: namespace key is snake_cased (dot-accessible)
        assert!(
            js.contains("codemode[\"arcane_mcp\"]"),
            "hyphenated upstream key must be snake_cased: {js}"
        );
        // ABSENCE: the raw hyphenated key must NOT be the namespace key
        assert!(
            !js.contains("codemode[\"arcane-mcp\"]"),
            "raw hyphenated key would only be bracket-accessible"
        );
        // PRESENCE: callTool id keeps the RAW upstream name (routing correctness)
        assert!(
            js.contains("upstream::arcane-mcp::arcane"),
            "callTool id must keep the raw upstream name: {js}"
        );
    }
}
