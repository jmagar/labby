//! Runtime JS proxy generation for Code Mode.
//!
//! Generates the executable `var codemode = {...}` proxy object that is sent
//! through the Code Mode Start protocol and injected into the sandbox after
//! `callTool` is defined. The proxy lets the agent call
//! `codemode.<namespace>.<tool>(params)`, which routes to
//! `callTool("<namespace>::<dotted.name>", params)`.
//!
//! This is RUNTIME JS, not a TypeScript declaration: it never enters the
//! model's context (unlike the deleted typed-preamble-in-tool-description
//! machinery from commit 780c67d3). Surfacing types/schemas via `search` is a
//! separate follow-up; this module only restores the executable proxy.

use super::types::{CodeModeCatalogKind, CodeModeDiscoveryEntry, ToolDescriptor};

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

/// Top-level `codemode` helper names owned by Lab's local runtime.
///
/// Namespaces that sanitize to one of these names are suffixed so a
/// real namespace named `search`, `describe`, or `step` cannot overwrite the
/// local discovery/control helpers.
const CODEMODE_TOP_LEVEL_RESERVED: &[&str] = &["search", "describe", "run", "step"];

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

/// Convert a namespace name to the runtime `codemode.<namespace>` key.
pub(crate) fn namespace_segment(name: &str) -> String {
    let namespace = tool_name_to_snake(name);
    if CODEMODE_TOP_LEVEL_RESERVED.contains(&namespace.as_str()) {
        format!("{namespace}_")
    } else {
        namespace
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
      [__codemodeNormalize(entry.namespace), 8],
      [__codemodeNormalize(entry.description), 5],
      [__codemodeNormalize((entry.tags || []).join(" ")), 7],
      [__codemodeNormalize(entry.kind === "snippet" ? "codemode run snippet" : ""), 9]
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
        kind: entry.kind,
        namespace: entry.namespace,
        name: entry.name,
        description: entry.description,
        signature: entry.signature,
        tags: entry.tags || [],
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
  var exact = [];
  var bare = [];
  var ambiguous = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    if (raw === entry.id || raw === entry.path || raw === entry.helper) exact.push(entry);
    if (entry.kind === "snippet" && raw === "snippet::" + entry.name) exact.push(entry);
    if (raw === entry.name) bare.push(entry);
    if (raw === entry.namespace) ambiguous.push(entry);
  }}
  if (exact.length > 1) {{
    var uniqueExact = [];
    var seenExact = {{}};
    for (var e = 0; e < exact.length; e++) {{
      if (!seenExact[exact[e].path]) {{
        seenExact[exact[e].path] = true;
        uniqueExact.push(exact[e]);
      }}
    }}
    exact = uniqueExact;
  }}
  if (!exact.length && bare.length === 1) exact = bare;
  if (!exact.length && (ambiguous.length || bare.length > 1)) {{
    var candidates = ambiguous.length ? ambiguous : bare;
    throw new Error(JSON.stringify({{
      kind: "ambiguous_target",
      message: "codemode.describe requires an exact id, path, helper, or unambiguous bare name",
      valid: candidates.map(function(item) {{ return item.path; }}).sort()
    }}));
  }}
  if (!exact.length) {{
    throw new Error(JSON.stringify({{ kind: "unknown_tool", message: "No Code Mode discovery target matched `" + raw + "`" }}));
  }}
  if (exact.length > 1) {{
    throw new Error(JSON.stringify({{
      kind: "ambiguous_target",
      message: "codemode.describe matched multiple targets",
      valid: exact.map(function(item) {{ return item.path; }}).sort()
    }}));
  }}
  var entry = exact[0];
  var markdown;
  if (entry.kind === "snippet") {{
    var inputLines = (entry.inputs || []).map(function(input) {{
      var bits = ["- `" + input.name + "` (" + input.ty + ")"];
      if (input.required) bits.push("required");
      if (Object.prototype.hasOwnProperty.call(input, "default")) bits.push("default: `" + JSON.stringify(input.default) + "`");
      if (input.description) bits.push(input.description);
      return bits.join(" - ");
    }}).join("\n");
    markdown = "# " + entry.name + "\n\nKind: snippet\n\nName: `" + entry.name + "`\n\nDescription: " + entry.description + "\n\nRun: `codemode.run(" + JSON.stringify(entry.name) + ", input)`\n" + (inputLines ? "\nInputs:\n" + inputLines + "\n" : "\nInputs: none\n");
  }} else {{
    markdown = "# " + entry.path + "\n\n" + entry.description + "\n\n- kind: `tool`\n- id: `" + entry.id + "`\n- helper: `" + entry.helper + "`\n- signature: `" + entry.signature + "`\n";
  }}
  return Promise.resolve({{
    path: entry.path,
    id: entry.id,
    kind: entry.kind,
    markdown: markdown
  }});
}};
codemode.run = function(name, input) {{
  return globalThis.__labRunSnippet(name, input == null ? {{}} : input);
}};
codemode.step = function(name, fn) {{
  if (typeof fn !== "function") throw new Error("codemode.step requires a function");
  return Promise.resolve().then(fn);
}};
"##
    ))
}

/// Generate a JavaScript preamble string that defines the `codemode` proxy
/// namespace, plus `codemode.__meta__.namespaces()` and a `__namespaces__`
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
pub(crate) fn generate_js_proxy_from_catalog(
    tools: &[&ToolDescriptor],
    namespaces: &[String],
) -> Result<String, String> {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let mut by_namespace: BTreeMap<&str, Vec<&ToolDescriptor>> = BTreeMap::new();
    for tool in tools {
        if tool.kind != CodeModeCatalogKind::Tool {
            continue;
        }
        by_namespace.entry(&tool.namespace).or_default().push(*tool);
    }

    let mut parts = String::new();
    let mut by_snake_namespace: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut final_proxy_keys: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    for (namespace_name, namespace_tools) in &by_namespace {
        let namespace_snake = namespace_segment(namespace_name);
        let mut snake_to_dotted: BTreeMap<String, String> = BTreeMap::new();
        let mut sorted_tools = namespace_tools.to_vec();
        sorted_tools.sort_by(|a, b| a.name.cmp(&b.name));
        for tool in &sorted_tools {
            let snake = tool_name_to_snake(&tool.name);
            if snake_to_dotted.contains_key(&snake) {
                let existing = snake_to_dotted
                    .get(&snake)
                    .map(String::as_str)
                    .unwrap_or("<unknown>");
                return Err(format!(
                    "Tool names \"{existing}\" and \"{}\" both sanitize to \"{snake}\" in namespace \"{namespace_name}\"",
                    tool.name
                ));
            }
            snake_to_dotted.insert(snake, tool.name.clone());
        }

        let method_defs = by_snake_namespace
            .entry(namespace_snake.clone())
            .or_default();
        for (snake, dotted) in &snake_to_dotted {
            if let Some((existing_namespace, existing_tool)) = final_proxy_keys.insert(
                (namespace_snake.clone(), snake.clone()),
                (namespace_name.to_string(), dotted.clone()),
            ) {
                return Err(format!(
                    "Tools \"{existing_namespace}::{existing_tool}\" and \"{namespace_name}::{dotted}\" both sanitize to \"{namespace_snake}.{snake}\""
                ));
            }
            let tool_id = super::types::namespaced_tool_id(namespace_name, dotted);
            let tool_id_json =
                serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"unknown\"".to_string());
            let snake_json =
                serde_json::to_string(snake.as_str()).unwrap_or_else(|_| format!("\"{snake}\""));
            method_defs.push(format!(
                "    {snake_json}: function(p) {{ return callTool({tool_id_json}, p == null ? {{}} : p); }}"
            ));
        }
    }

    for (namespace_snake, method_defs) in &by_snake_namespace {
        let namespace_snake_json =
            serde_json::to_string(namespace_snake).unwrap_or_else(|_| "\"unknown\"".to_string());
        let methods = method_defs.join(",\n");
        let _ = write!(
            parts,
            "codemode[{namespace_snake_json}] = {{\n{methods}\n}};\n"
        );
    }

    let namespaces_json = serde_json::to_string(namespaces).unwrap_or_else(|_| "[]".to_string());

    Ok(format!(
        "// Code Mode proxy — auto-generated\n\
         globalThis.codemode = globalThis.codemode || {{}};\n\
         var codemode = globalThis.codemode;\n\
         codemode.run = function(name, input) {{ return globalThis.__labRunSnippet(name, input == null ? {{}} : input); }};\n\
         {parts}\
         codemode.__meta__ = {{ namespaces: function() {{ return Promise.resolve({namespaces_json}); }} }};\n\
         var __namespaces__ = {namespaces_json};\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolDescriptor;

    fn discovery_entry(namespace: &str, name: &str, description: &str) -> CodeModeDiscoveryEntry {
        let path = format!("{namespace}.{name}");
        CodeModeDiscoveryEntry {
            kind: CodeModeCatalogKind::Tool,
            id: format!("{namespace}::{name}"),
            path: path.clone(),
            namespace: namespace.to_string(),
            name: name.to_string(),
            helper: format!("codemode.{path}"),
            description: description.to_string(),
            signature: format!("codemode.{path}(params: unknown): Promise<unknown>"),
            tags: Vec::new(),
            inputs: Vec::new(),
        }
    }

    /// Build a `ToolDescriptor` for the proxy-generation tests.
    fn descriptor(namespace: &str, tool: &str) -> ToolDescriptor {
        ToolDescriptor::tool(namespace, tool, "", None, None)
    }

    /// Generate the runtime proxy from owned descriptors.
    fn proxy(tools: &[ToolDescriptor], namespaces: &[String]) -> Result<String, String> {
        let refs: Vec<&ToolDescriptor> = tools.iter().collect();
        generate_js_proxy_from_catalog(&refs, namespaces)
    }

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

    #[test]
    fn namespace_segment_preserves_top_level_helpers() {
        assert_eq!(namespace_segment("github"), "github");
        assert_eq!(namespace_segment("search"), "search_");
        assert_eq!(namespace_segment("describe"), "describe_");
        assert_eq!(namespace_segment("step"), "step_");
    }

    // ── generate_js_proxy ─────────────────────────────────────────────────────

    #[test]
    fn discovery_preamble_preserves_existing_codemode_object() {
        let entries = vec![discovery_entry("arcane", "containers", "List containers")];
        let js = generate_discovery_js(&entries).expect("js");
        assert!(js.contains("globalThis.codemode = globalThis.codemode || {}"));
        assert!(js.contains("codemode.search"));
        assert!(js.contains("codemode.describe"));
        assert!(!js.contains("schema"));
        assert!(!js.contains("output_schema"));
        assert!(!js.contains("dts"));
    }

    #[test]
    fn discovery_preamble_advertises_search_describe_and_step_semantics() {
        let entries = vec![
            discovery_entry("github", "search_issues", "Search GitHub issues"),
            discovery_entry("github", "list_pull_requests", "List GitHub pull requests"),
        ];
        let js = generate_discovery_js(&entries).expect("js");

        assert!(js.contains("typeof input === \"object\""));
        assert!(js.contains("Math.max(1, Math.min(50"));
        assert!(js.contains("truncated: total > limit"));
        assert!(js.contains("raw === entry.id || raw === entry.path || raw === entry.helper"));
        assert!(js.contains("ambiguous_target"));
        assert!(js.contains("unknown_tool"));
        assert!(js.contains("Promise.resolve().then(fn)"));
    }

    #[test]
    fn discovery_describe_rejects_namespace_only_targets() {
        let entries = vec![discovery_entry("github", "search_issues", "Search issues")];
        let js = generate_discovery_js(&entries).expect("js");
        assert!(js.contains("ambiguous_target"));
        assert!(js.contains("github.search_issues"));
    }

    #[test]
    fn generate_js_proxy_quoted_keys_tolerate_special_chars() {
        // Tool with a slash in the name — previously caused a QuickJS syntax
        // error because the unquoted key was invalid.
        let tool = descriptor("github", "create/issue");
        let js = proxy(&[tool], &["github".to_string()]).expect("proxy");

        // PRESENCE: the namespace object must be present
        assert!(
            js.contains("codemode[\"github\"]"),
            "namespace block missing"
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
        let tool = descriptor("radarr", "movie.search");
        let js = proxy(&[tool], &["radarr".to_string()]).expect("proxy");

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
        // PRESENCE: snake_case method routes to the dotted tool id
        assert!(
            js.contains("radarr::movie.search"),
            "method must route to dotted tool id"
        );
        // PRESENCE: __meta__.namespaces reflects the namespace list
        assert!(
            js.contains("[\"radarr\"]"),
            "namespaces list must be embedded"
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
    fn generate_js_proxy_does_not_overwrite_local_discovery_helpers() {
        for raw in ["search", "describe", "step"] {
            let namespace = namespace_segment(raw);
            let tool = descriptor(raw, "lookup");
            let js = proxy(&[tool], &[raw.to_string()]).expect("proxy");

            assert!(
                js.contains(&format!("codemode[\"{namespace}\"]")),
                "reserved namespace must be suffixed: {js}"
            );
            assert!(
                !js.contains(&format!("codemode[\"{raw}\"] = {{")),
                "reserved namespace must not replace codemode.{raw}"
            );
            assert!(
                js.contains(&format!("{raw}::lookup")),
                "raw id must still route to the original namespace"
            );
        }
    }

    #[test]
    fn generate_js_proxy_snake_cases_hyphenated_namespace_keys() {
        // Hyphenated namespaces (arcane-mcp, github-chat, …) must be reachable
        // via dot notation `codemode.arcane_mcp.tool(...)`; the callTool id keeps
        // the RAW namespace name so the host routes to the real source.
        let tool = descriptor("arcane-mcp", "arcane");
        let js = proxy(&[tool], &["arcane-mcp".to_string()]).expect("proxy");

        assert!(
            js.contains("codemode[\"arcane_mcp\"]"),
            "hyphenated namespace key must be snake_cased: {js}"
        );
        assert!(
            !js.contains("codemode[\"arcane-mcp\"]"),
            "raw hyphenated key would only be bracket-accessible"
        );
        assert!(
            js.contains("arcane-mcp::arcane"),
            "callTool id must keep the raw namespace name: {js}"
        );
    }

    #[test]
    fn generate_js_proxy_rejects_sanitized_tool_collisions() {
        let dotted = descriptor("demo", "movie.search");
        let underscored = descriptor("demo", "movie_search");

        let err = proxy(&[dotted, underscored], &["demo".to_string()])
            .expect_err("sanitized collisions must not be last-wins");

        assert!(err.contains("both sanitize to"));
    }

    #[test]
    fn generate_js_proxy_rejects_final_proxy_collisions_across_raw_namespaces() {
        let hyphenated = descriptor("foo-bar", "ping");
        let dotted = descriptor("foo.bar", "ping");

        let err = proxy(
            &[hyphenated, dotted],
            &["foo-bar".to_string(), "foo.bar".to_string()],
        )
        .expect_err("final proxy collisions must not generate duplicate keys");

        assert!(err.contains("both sanitize to"));
        assert!(err.contains("foo_bar"));
        assert!(err.contains("ping"));
    }
}
