//! Runtime JS proxy generation for Code Mode.
//!
//! Generates the executable `var codemode = {...}` proxy object that is sent
//! through the Code Mode Start protocol and injected into the sandbox after
//! `callTool` is defined. The proxy lets the agent call
//! `codemode.<upstream>.<tool>(params)`, which routes to
//! `callTool("<upstream>::<dotted.name>", params)`.
//!
//! This is RUNTIME JS, not a TypeScript declaration: it never enters the
//! model's context (unlike the deleted typed-preamble-in-tool-description
//! machinery from commit 780c67d3). Surfacing types/schemas via `search` is a
//! separate follow-up; this module only restores the executable proxy.

use super::types::CodeModeDiscoveryEntry;
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
    "abstract",
    "arguments",
    "await",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "function",
    "goto",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "int",
    "interface",
    "let",
    "long",
    "native",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "volatile",
    "while",
    "with",
    "yield",
];

/// Convert a dotted/hyphenated/slashed/coloned tool name to snake_case.
///
/// Examples (Cloudflare parity):
/// - `movie.search` → `movie_search`
/// - `tv-show.get` → `tv_show_get`
/// - `create/issue` → `createissue`
/// - `list:repos` → `listrepos`
/// - `delete` → `delete_` (reserved word)
/// - `2fa_setup` → `_2fa_setup` (leading digit prefixed with `_`)
///
/// KNOWN COLLISION: `movie.search` and `movie_search` both map to `movie_search`
/// — last insert wins when building the namespace. A `tracing::debug!` is emitted
/// when a collision is detected.
pub fn tool_name_to_snake(name: &str) -> String {
    if name.is_empty() {
        return "_".to_string();
    }

    // Cloudflare parity: replace common separators with underscores, then strip
    // any remaining characters that are invalid in JavaScript identifiers.
    let mut snake = String::new();
    let mut previous_was_separator = false;
    for ch in name.chars() {
        if ch == '-' || ch == '.' || ch.is_whitespace() {
            if !previous_was_separator {
                snake.push('_');
            }
            previous_was_separator = true;
        } else if ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            snake.push(ch);
            previous_was_separator = false;
        } else {
            previous_was_separator = false;
        }
    }
    let snake = snake.trim_matches('_').to_string();
    let snake = if snake.is_empty() {
        "_".to_string()
    } else {
        snake
    };

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

pub(crate) fn generate_discovery_js(entries: &[CodeModeDiscoveryEntry]) -> Result<String, String> {
    let json = serde_json::to_string(entries)
        .map_err(|err| format!("failed to serialize Code Mode discovery catalog: {err}"))?;
    Ok(format!(
        r##"
globalThis.codemode = globalThis.codemode || {{}};
var codemode = globalThis.codemode;
var __codemodeDiscovery = {json};
function __codemodeNormalize(value) {{
  return String(value == null ? "" : value)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}}
function __codemodeTokens(value) {{
  var normalized = __codemodeNormalize(value);
  return normalized ? normalized.split(/\s+/g) : [];
}}
codemode.search = function(input) {{
  var query = typeof input === "object" && input !== null ? String(input.query || "") : String(input || "");
  var limit = typeof input === "object" && input !== null && Number.isFinite(Number(input.limit))
    ? Math.max(1, Math.min(50, Number(input.limit)))
    : 50;
  var tokens = __codemodeTokens(query);
  if (!tokens.length) return Promise.resolve({{ results: [], total: 0, truncated: false }});
  var scored = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    var fields = [
      [__codemodeNormalize(entry.path), 12],
      [__codemodeNormalize(entry.name), 10],
      [__codemodeNormalize(entry.upstream), 8],
      [__codemodeNormalize(entry.description), 5]
    ];
    var covered = 0;
    var score = 0;
    for (var t = 0; t < tokens.length; t++) {{
      var tokenScore = 0;
      for (var f = 0; f < fields.length; f++) {{
        if (fields[f][0].indexOf(tokens[t]) !== -1 && fields[f][1] > tokenScore) tokenScore = fields[f][1];
      }}
      if (tokenScore > 0) {{
        covered++;
        score += tokenScore;
      }}
    }}
    var required = tokens.length <= 2 ? tokens.length : Math.ceil(tokens.length * 0.6);
    if (covered >= required) {{
      scored.push({{
        path: entry.path,
        id: entry.id,
        upstream: entry.upstream,
        name: entry.name,
        description: entry.description,
        signature: entry.signature,
        score: score
      }});
    }}
  }}
  scored.sort(function(a, b) {{
    if (b.score !== a.score) return b.score - a.score;
    return a.path < b.path ? -1 : a.path > b.path ? 1 : 0;
  }});
  var total = scored.length;
  return Promise.resolve({{ results: scored.slice(0, limit), total: total, truncated: total > limit }});
}};
codemode.describe = function(target) {{
  var raw = String(target == null ? "" : target).trim();
  var matches = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    if (raw === entry.id || raw === entry.path || raw === entry.helper) matches.push(entry);
    if (raw === entry.upstream) matches.push({{ __ambiguous: true, path: entry.path }});
  }}
  var ambiguous = matches.filter(function(item) {{ return item.__ambiguous; }});
  if (ambiguous.length) {{
    throw new Error(JSON.stringify({{
      kind: "ambiguous_target",
      message: "codemode.describe requires an exact tool id, upstream.tool path, or helper path",
      valid: ambiguous.map(function(item) {{ return item.path; }}).sort()
    }}));
  }}
  if (!matches.length) {{
    throw new Error(JSON.stringify({{ kind: "unknown_tool", message: "No Code Mode discovery target matched `" + raw + "`" }}));
  }}
  var entry = matches[0];
  return Promise.resolve({{
    path: entry.path,
    id: entry.id,
    markdown: "# " + entry.path + "\n\n" + entry.description + "\n\n- id: `" + entry.id + "`\n- helper: `" + entry.helper + "`\n- signature: `" + entry.signature + "`\n"
  }});
}};
codemode.step = function(name, fn) {{
  if (typeof fn !== "function") throw new Error("codemode.step requires a function");
  return Promise.resolve().then(fn);
}};
"##
    ))
}

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
pub fn generate_js_proxy(tools: &[UpstreamTool], upstreams: &[String]) -> Result<String, String> {
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
    let mut final_proxy_keys: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    for (upstream_name, upstream_tools) in &by_upstream {
        let upstream_snake = tool_name_to_snake(upstream_name);

        // Build snake_case → dotted name mapping, last registration wins on collision.
        let mut snake_to_dotted: BTreeMap<String, String> = BTreeMap::new();
        let mut sorted_tools = upstream_tools.to_vec();
        sorted_tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));
        for tool in &sorted_tools {
            let snake = tool_name_to_snake(tool.tool.name.as_ref());
            if snake_to_dotted.contains_key(&snake) {
                let existing = snake_to_dotted
                    .get(&snake)
                    .map(String::as_str)
                    .unwrap_or("<unknown>");
                return Err(format!(
                    "Tool names \"{existing}\" and \"{}\" both sanitize to \"{snake}\" in upstream \"{upstream_name}\"",
                    tool.tool.name.as_ref()
                ));
            }
            snake_to_dotted.insert(snake, tool.tool.name.to_string());
        }

        let method_defs = by_snake_upstream.entry(upstream_snake.clone()).or_default();
        for (snake, dotted) in &snake_to_dotted {
            if let Some((existing_upstream, existing_tool)) = final_proxy_keys.insert(
                (upstream_snake.clone(), snake.clone()),
                (upstream_name.to_string(), dotted.clone()),
            ) {
                return Err(format!(
                    "Tools \"{existing_upstream}::{existing_tool}\" and \"{upstream_name}::{dotted}\" both sanitize to \"{upstream_snake}.{snake}\""
                ));
            }
            // callTool id uses the RAW upstream + RAW tool name.
            let tool_id = super::types::upstream_tool_id(upstream_name, &dotted);
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

    Ok(format!(
        "// Code Mode proxy — auto-generated\n\
         globalThis.codemode = globalThis.codemode || {{}};\n\
         var codemode = globalThis.codemode;\n\
         {parts}\
         codemode.__meta__ = {{ upstreams: function() {{ return Promise.resolve({upstreams_json}); }} }};\n\
         var __upstreams__ = {upstreams_json};\n"
    ))
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
        assert_eq!(tool_name_to_snake("create/issue"), "createissue");
        assert_eq!(tool_name_to_snake("list:repos"), "listrepos");
        assert_eq!(
            tool_name_to_snake("repos/create/branch"),
            "reposcreatebranch"
        );
        // PRESENCE: already-snake input is preserved
        assert_eq!(tool_name_to_snake("create_issue"), "create_issue");
        // PRESENCE: leading digit gets prefixed with underscore
        assert_eq!(tool_name_to_snake("2fa_setup"), "_2fa_setup");
        // ABSENCE: separators must not appear in output (would break JS syntax)
        assert!(!tool_name_to_snake("create/issue").contains('/'));
        assert!(!tool_name_to_snake("list:repos").contains(':'));
    }

    #[test]
    fn tool_name_to_snake_matches_cloudflare_identifier_sanitization() {
        assert_eq!(tool_name_to_snake("list tags"), "list_tags");
        assert_eq!(tool_name_to_snake("create#issue!"), "createissue");
        assert_eq!(tool_name_to_snake("await"), "await_");
        assert_eq!(tool_name_to_snake(""), "_");
    }

    // ── generate_js_proxy ─────────────────────────────────────────────────────

    #[test]
    fn discovery_preamble_preserves_existing_codemode_object() {
        let entries = vec![CodeModeDiscoveryEntry {
            id: "arcane::containers".to_string(),
            path: "arcane.containers".to_string(),
            upstream: "arcane".to_string(),
            name: "containers".to_string(),
            helper: "codemode.arcane.containers".to_string(),
            description: "List containers".to_string(),
            signature: "codemode.arcane.containers(params: unknown): Promise<unknown>".to_string(),
        }];
        let js = generate_discovery_js(&entries).expect("js");
        assert!(js.contains("globalThis.codemode = globalThis.codemode || {}"));
        assert!(js.contains("codemode.search"));
        assert!(js.contains("codemode.describe"));
        assert!(!js.contains("schema"));
        assert!(!js.contains("output_schema"));
        assert!(!js.contains("dts"));
    }

    #[test]
    fn discovery_describe_rejects_upstream_only_targets() {
        let entries = vec![CodeModeDiscoveryEntry {
            id: "github::search_issues".to_string(),
            path: "github.search_issues".to_string(),
            upstream: "github".to_string(),
            name: "search_issues".to_string(),
            helper: "codemode.github.search_issues".to_string(),
            description: "Search issues".to_string(),
            signature: "codemode.github.search_issues(params: unknown): Promise<unknown>"
                .to_string(),
        }];
        let js = generate_discovery_js(&entries).expect("js");
        assert!(js.contains("ambiguous_target"));
        assert!(js.contains("github.search_issues"));
    }

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
            output_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["github".to_string()]).expect("proxy");

        // PRESENCE: the upstream object must be present
        assert!(
            js.contains("codemode[\"github\"]"),
            "upstream block missing"
        );
        // PRESENCE: the tool key must appear as a quoted string (not unquoted)
        assert!(
            js.contains("\"createissue\""),
            "sanitized key must be quoted"
        );
        // ABSENCE: no unquoted slash-containing key that would break JS syntax
        assert!(
            !js.contains("create/issue:"),
            "slash in unquoted key would break JS"
        );
        // PRESENCE: callTool must be wired to the original dotted tool id
        assert!(
            js.contains("github::create/issue"),
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
            output_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["radarr".to_string()]).expect("proxy");

        // PRESENCE: preserves the platform-created codemode object
        assert!(
            js.contains("globalThis.codemode = globalThis.codemode || {}"),
            "must preserve codemode object"
        );
        assert!(
            js.contains("var codemode = globalThis.codemode"),
            "must bind preserved codemode object"
        );
        assert!(
            !js.contains("var codemode = {}"),
            "must not replace codemode"
        );
        // PRESENCE: snake_case method routes to the dotted upstream id
        assert!(
            js.contains("radarr::movie.search"),
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
            output_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["arcane-mcp".to_string()]).expect("proxy");

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
            js.contains("arcane-mcp::arcane"),
            "callTool id must keep the raw upstream name: {js}"
        );
    }

    #[test]
    fn generate_js_proxy_rejects_sanitized_tool_collisions() {
        let schema = Arc::new(serde_json::Map::new());
        let dotted = UpstreamTool {
            upstream_name: Arc::from("demo"),
            tool: Tool::new("movie.search", "Search", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
            output_schema: None,
        };
        let underscored = UpstreamTool {
            upstream_name: Arc::from("demo"),
            tool: Tool::new("movie_search", "Search", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
            output_schema: None,
        };

        let err = generate_js_proxy(&[dotted, underscored], &["demo".to_string()])
            .expect_err("sanitized collisions must not be last-wins");

        assert!(err.contains("both sanitize to"));
    }

    #[test]
    fn generate_js_proxy_rejects_final_proxy_collisions_across_raw_upstreams() {
        let schema = Arc::new(serde_json::Map::new());
        let hyphenated = UpstreamTool {
            upstream_name: Arc::from("foo-bar"),
            tool: Tool::new("ping", "Ping", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
            output_schema: None,
        };
        let dotted = UpstreamTool {
            upstream_name: Arc::from("foo.bar"),
            tool: Tool::new("ping", "Ping", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
            output_schema: None,
        };

        let err = generate_js_proxy(
            &[hyphenated, dotted],
            &["foo-bar".to_string(), "foo.bar".to_string()],
        )
        .expect_err("final proxy collisions must not generate duplicate keys");

        assert!(err.contains("both sanitize to"));
        assert!(err.contains("foo_bar"));
        assert!(err.contains("ping"));
    }
}
