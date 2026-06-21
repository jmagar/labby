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

use super::types::{CodeModeCatalogEntry, CodeModeCatalogKind, CodeModeDiscoveryEntry};

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
/// Upstream namespaces that sanitize to one of these names are suffixed so a
/// real upstream named `search`, `describe`, or `step` cannot overwrite the
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

/// Convert an upstream name to the runtime `codemode.<namespace>` key.
pub(crate) fn upstream_name_to_namespace(name: &str) -> String {
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
    // Compact search index: the `#[serde(skip)]` on `schema`/`dts` keeps the
    // per-execute preamble small. `codemode.search` iterates only this array.
    let json = serde_json::to_string(entries)
        .map_err(|err| format!("failed to serialize Code Mode discovery catalog: {err}"))?;
    // Separate, describe-only lookup keyed by tool id. We emit just the already
    // generated `.d.ts` type body (a TypeScript declaration string) — NOT the
    // raw JSON schema — so `codemode.describe` can show field names/types
    // without bloating the search index above. Snippets (empty type body) are
    // skipped. Built as a plain JS object literal via serde for correct
    // escaping of the embedded type text.
    let types_map: serde_json::Map<String, serde_json::Value> = entries
        .iter()
        .filter(|entry| !entry.dts.is_empty())
        .map(|entry| {
            (
                entry.id.clone(),
                serde_json::Value::String(entry.dts.clone()),
            )
        })
        .collect();
    let types_json = serde_json::to_string(&serde_json::Value::Object(types_map))
        .map_err(|err| format!("failed to serialize Code Mode type lookup: {err}"))?;
    Ok(format!(
        r##"
globalThis.codemode = globalThis.codemode || {{}};
var codemode = globalThis.codemode;
var __codemodeDiscovery = {json};
var __codemodeTypes = {types_json};
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
  var __codemodeNoMatchHint = "No matches. Broaden or try synonyms, or call codemode.__meta__.upstreams() to list namespaces and search by upstream name.";
  if (!tokens.length) return Promise.resolve({{ results: [], total: 0, truncated: false, hint: __codemodeNoMatchHint }});
  var scored = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    var fields = [
      [__codemodeNormalize(entry.path), 12],
      [__codemodeNormalize(entry.name), 10],
      [__codemodeNormalize(entry.upstream), 8],
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
        upstream: entry.upstream,
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
  if (total === 0) {{
    return Promise.resolve({{ results: [], total: 0, truncated: false, hint: __codemodeNoMatchHint }});
  }}
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
    if (raw === entry.upstream) ambiguous.push(entry);
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
    var typeBody = __codemodeTypes[entry.id];
    if (typeBody) {{
      markdown += "\nParameters (TypeScript):\n\n```typescript\n" + typeBody + "```\n";
    }}
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

pub(crate) fn generate_js_proxy_from_catalog(
    tools: &[&CodeModeCatalogEntry],
    upstreams: &[String],
) -> Result<String, String> {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let mut by_upstream: BTreeMap<&str, Vec<&CodeModeCatalogEntry>> = BTreeMap::new();
    for tool in tools {
        if tool.kind != CodeModeCatalogKind::Tool {
            continue;
        }
        by_upstream.entry(&tool.upstream).or_default().push(*tool);
    }

    let mut parts = String::new();
    let mut by_snake_upstream: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut final_proxy_keys: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    for (upstream_name, upstream_tools) in &by_upstream {
        let upstream_snake = upstream_name_to_namespace(upstream_name);
        let mut snake_to_dotted: BTreeMap<String, String> = BTreeMap::new();
        let mut sorted_tools = upstream_tools.to_vec();
        sorted_tools.sort_by(|a, b| a.name.cmp(&b.name));
        for tool in &sorted_tools {
            let snake = tool_name_to_snake(&tool.name);
            if snake_to_dotted.contains_key(&snake) {
                let existing = snake_to_dotted
                    .get(&snake)
                    .map(String::as_str)
                    .unwrap_or("<unknown>");
                return Err(format!(
                    "Tool names \"{existing}\" and \"{}\" both sanitize to \"{snake}\" in upstream \"{upstream_name}\"",
                    tool.name
                ));
            }
            snake_to_dotted.insert(snake, tool.name.clone());
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
            let tool_id = super::types::upstream_tool_id(upstream_name, dotted);
            let tool_id_json =
                serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"unknown\"".to_string());
            let snake_json =
                serde_json::to_string(snake.as_str()).unwrap_or_else(|_| format!("\"{snake}\""));
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

    let upstreams_json = serde_json::to_string(upstreams).unwrap_or_else(|_| "[]".to_string());

    Ok(format!(
        "// Code Mode proxy — auto-generated\n\
         globalThis.codemode = globalThis.codemode || {{}};\n\
         var codemode = globalThis.codemode;\n\
         codemode.run = function(name, input) {{ return globalThis.__labRunSnippet(name, input == null ? {{}} : input); }};\n\
         {parts}\
         codemode.__meta__ = {{ upstreams: function() {{ return Promise.resolve({upstreams_json}); }} }};\n\
         var __upstreams__ = {upstreams_json};\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovery_entry(upstream: &str, name: &str, description: &str) -> CodeModeDiscoveryEntry {
        let path = format!("{upstream}.{name}");
        CodeModeDiscoveryEntry {
            kind: CodeModeCatalogKind::Tool,
            id: format!("{upstream}::{name}"),
            path: path.clone(),
            upstream: upstream.to_string(),
            name: name.to_string(),
            helper: format!("codemode.{path}"),
            description: description.to_string(),
            signature: format!("codemode.{path}(params: unknown): Promise<unknown>"),
            tags: Vec::new(),
            inputs: Vec::new(),
            schema: None,
            dts: String::new(),
        }
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
    fn upstream_name_to_namespace_preserves_top_level_helpers() {
        assert_eq!(upstream_name_to_namespace("github"), "github");
        assert_eq!(upstream_name_to_namespace("search"), "search_");
        assert_eq!(upstream_name_to_namespace("describe"), "describe_");
        assert_eq!(upstream_name_to_namespace("step"), "step_");
    }

    // ── generate_js_proxy_from_catalog ─────────────────────────────────────────────────────

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
    fn discovery_describe_rejects_upstream_only_targets() {
        let entries = vec![discovery_entry("github", "search_issues", "Search issues")];
        let js = generate_discovery_js(&entries).expect("js");
        assert!(js.contains("ambiguous_target"));
        assert!(js.contains("github.search_issues"));
    }

    #[test]
    fn discovery_describe_surfaces_tool_type_body() {
        // A multi-field tool's `.d.ts` type body must reach `codemode.describe`
        // so the agent can construct valid params without an execute round-trip.
        let mut entry = discovery_entry("github", "list_tags", "List repository tags");
        entry.dts =
            "type GithubListTagsInput = { owner: string; repo: string; perPage?: number };\n"
                .to_string();
        let js = generate_discovery_js(&[entry]).expect("js");

        // PRESENCE: the type body is embedded in the describe-only lookup map,
        // keyed by tool id, and the describe branch renders it as a fenced block.
        assert!(
            js.contains("__codemodeTypes"),
            "describe-only type lookup map must be emitted"
        );
        assert!(
            js.contains("GithubListTagsInput"),
            "type body must be present for describe lookup"
        );
        assert!(
            js.contains("github::list_tags"),
            "type lookup must be keyed by the tool id"
        );
        // PRESENCE: the describe markdown renders the looked-up body in a fence.
        assert!(
            js.contains("```typescript"),
            "describe must render the type body in a typescript fence"
        );
        // PRESENCE: every input field name reaches the model.
        assert!(js.contains("owner"));
        assert!(js.contains("repo"));
        assert!(js.contains("perPage"));
    }

    #[test]
    fn discovery_search_index_stays_lean_when_tools_carry_types() {
        // The compact `__codemodeDiscovery` search array must NOT carry the raw
        // input schema even when the catalog entry holds one — embedding it
        // would balloon the per-execute preamble. The type body lives only in
        // the separate describe lookup.
        let mut entry = discovery_entry("github", "list_tags", "List repository tags");
        entry.schema = Some(serde_json::json!({
            "type": "object",
            "properties": { "owner": { "type": "string" } },
            "required": ["owner"],
        }));
        entry.dts = "type GithubListTagsInput = { owner: string };\n".to_string();
        let js = generate_discovery_js(&[entry]).expect("js");

        // The discovery array slice (before the type lookup) must not contain a
        // serialized `properties`/`required` JSON schema object — those only
        // appear via the omitted raw schema, which we never emit anywhere.
        let discovery_slice = js
            .split("var __codemodeTypes")
            .next()
            .expect("discovery array precedes type lookup");
        assert!(
            !discovery_slice.contains("\"properties\""),
            "search index must not embed the raw input schema"
        );
        assert!(
            !discovery_slice.contains("\"required\""),
            "search index must not embed schema required-ness"
        );
        // ABSENCE (whole proxy): the raw schema keys never leak into the JS.
        assert!(
            !js.contains("\"properties\""),
            "raw input schema must not be emitted anywhere in the proxy"
        );
    }

    #[test]
    fn discovery_search_zero_result_returns_actionable_hint() {
        // A vocabulary mismatch must not be a silent empty array: the search
        // return carries a `hint` pointing the agent at namespace browsing.
        let entries = vec![discovery_entry(
            "github",
            "list_tags",
            "List repository tags",
        )];
        let js = generate_discovery_js(&entries).expect("js");

        // PRESENCE: a hint string is wired into the zero-result paths.
        assert!(
            js.contains("hint: __codemodeNoMatchHint"),
            "zero-result search must attach a hint field"
        );
        assert!(
            js.contains("codemode.__meta__.upstreams()"),
            "hint must point at namespace browsing"
        );
        // PRESENCE: both the empty-token guard and the post-scoring empty case
        // return the hint (two `total: 0` returns carry it).
        assert_eq!(
            js.matches("hint: __codemodeNoMatchHint").count(),
            2,
            "both empty-token and zero-score paths must return the hint"
        );
    }

    #[test]
    fn generate_js_proxy_quoted_keys_tolerate_special_chars() {
        // Tool with a slash in the name — previously caused "Exception generated
        // by QuickJS" because the unquoted key was a JS syntax error.
        let tool = CodeModeCatalogEntry::upstream_tool(
            "github",
            "create/issue",
            "Create an issue",
            None,
            None,
        );
        let js = generate_js_proxy_from_catalog(&[&tool], &["github".to_string()]).expect("proxy");

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
        let tool = CodeModeCatalogEntry::upstream_tool(
            "radarr",
            "movie.search",
            "Search for movies",
            None,
            None,
        );
        let js = generate_js_proxy_from_catalog(&[&tool], &["radarr".to_string()]).expect("proxy");

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
    fn generate_js_proxy_does_not_overwrite_local_discovery_helpers() {
        for upstream in ["search", "describe", "step"] {
            let namespace = upstream_name_to_namespace(upstream);
            let tool = CodeModeCatalogEntry::upstream_tool(
                upstream,
                "lookup",
                &format!("Lookup through {upstream} upstream"),
                None,
                None,
            );
            let js =
                generate_js_proxy_from_catalog(&[&tool], &[upstream.to_string()]).expect("proxy");

            assert!(
                js.contains(&format!("codemode[\"{namespace}\"]")),
                "reserved upstream must be suffixed: {js}"
            );
            assert!(
                !js.contains(&format!("codemode[\"{upstream}\"] = {{")),
                "reserved upstream must not replace codemode.{upstream}"
            );
            assert!(
                js.contains(&format!("{upstream}::lookup")),
                "raw upstream id must still route to the original upstream"
            );
        }
    }

    #[test]
    fn generate_js_proxy_snake_cases_hyphenated_upstream_keys() {
        // Hyphenated upstreams (arcane-mcp, github-chat, …) must be reachable via
        // dot notation `codemode.arcane_mcp.tool(...)`, not just bracket access —
        // `codemode.arcane-mcp` parses as subtraction. The callTool id must keep
        // the RAW upstream name so the gateway routes to the real server.
        let tool = CodeModeCatalogEntry::upstream_tool(
            "arcane-mcp",
            "arcane",
            "Docker management",
            None,
            None,
        );
        let js =
            generate_js_proxy_from_catalog(&[&tool], &["arcane-mcp".to_string()]).expect("proxy");

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
        let dotted =
            CodeModeCatalogEntry::upstream_tool("demo", "movie.search", "Search", None, None);
        let underscored =
            CodeModeCatalogEntry::upstream_tool("demo", "movie_search", "Search", None, None);

        let err = generate_js_proxy_from_catalog(&[&dotted, &underscored], &["demo".to_string()])
            .expect_err("sanitized collisions must not be last-wins");

        assert!(err.contains("both sanitize to"));
    }

    #[test]
    fn generate_js_proxy_rejects_final_proxy_collisions_across_raw_upstreams() {
        let hyphenated = CodeModeCatalogEntry::upstream_tool("foo-bar", "ping", "Ping", None, None);
        let dotted = CodeModeCatalogEntry::upstream_tool("foo.bar", "ping", "Ping", None, None);

        let err = generate_js_proxy_from_catalog(
            &[&hyphenated, &dotted],
            &["foo-bar".to_string(), "foo.bar".to_string()],
        )
        .expect_err("final proxy collisions must not generate duplicate keys");

        assert!(err.contains("both sanitize to"));
        assert!(err.contains("foo_bar"));
        assert!(err.contains("ping"));
    }
}
