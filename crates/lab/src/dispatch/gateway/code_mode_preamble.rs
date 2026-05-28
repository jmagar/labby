//! TypeScript declaration preamble generation for Code Mode.
//!
//! Generates `declare namespace codemode { ... }` from the live upstream tool catalog,
//! cached keyed on an aggregate hash of all upstream catalog hashes.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use dashmap::DashMap;
use serde_json::Value;

use crate::dispatch::upstream::types::UpstreamTool;

// ────────────────────────────────────────────────────────────────────────────
// ScopeTier — keying axis for the preamble cache
// ────────────────────────────────────────────────────────────────────────────

/// Scope-derived tier used as a cache-key axis.
///
/// `healthy_tools()` returns the same set for all callers — tool visibility is
/// not scope-filtered. We keep the tier axis for future correctness if that
/// invariant changes; callers with `lab:read` scope reach `Read`, `lab` scope
/// reaches `Execute`, and `lab:admin`/`TrustedLocal` reaches `Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeTier {
    /// `TrustedLocal` or `lab:admin` — full access
    Admin,
    /// `lab` scope — execution allowed
    Execute,
    /// `lab:read` scope — catalog read only
    Read,
}

// ────────────────────────────────────────────────────────────────────────────
// Aggregate catalog hash
// ────────────────────────────────────────────────────────────────────────────

/// A `(upstream_name, catalog_hash)` pair contributed by a single upstream.
#[derive(Debug, Clone)]
pub struct UpstreamCatalogHash {
    pub upstream: String,
    pub hash: u64,
}

/// Deterministically combine per-upstream catalog hashes into a single `u64`.
///
/// Upstreams are sorted by name before hashing so the aggregate is
/// order-independent.
pub fn aggregate_catalog_hash(upstreams: &[UpstreamCatalogHash]) -> u64 {
    let mut sorted: Vec<&UpstreamCatalogHash> = upstreams.iter().collect();
    sorted.sort_by(|a, b| a.upstream.cmp(&b.upstream));

    let mut hasher = DefaultHasher::new();
    for u in sorted {
        u.upstream.hash(&mut hasher);
        u.hash.hash(&mut hasher);
    }
    hasher.finish()
}

// ────────────────────────────────────────────────────────────────────────────
// Preamble cache
// ────────────────────────────────────────────────────────────────────────────

/// Cached entry holding both the generated TypeScript preamble and its tools JSON.
///
/// Returned together so cache hits avoid any pool fetch entirely.
#[derive(Debug, Clone)]
pub struct CachedPreamble {
    pub preamble: String,
    pub tools_json: Value,
}

/// Thread-safe LRU-free cache for generated preamble strings and their associated tools JSON.
///
/// Key: `(aggregate_catalog_hash, ScopeTier)`.
/// On a cold pool (aggregate == 0) callers get a cache miss and fall through to
/// generate a minimal/empty preamble.
#[derive(Debug, Default)]
pub struct PreambleCache {
    inner: DashMap<(u64, ScopeTier), CachedPreamble>,
}

impl PreambleCache {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    /// Look up a cached preamble and its tools JSON.
    ///
    /// Returns `CachedPreamble` so cache hits avoid any pool fetch.
    pub fn get(&self, aggregate: u64, tier: ScopeTier) -> Option<CachedPreamble> {
        self.inner
            .get(&(aggregate, tier))
            .map(|entry| entry.value().clone())
    }

    /// Insert a generated preamble and its tools JSON.
    pub fn insert(&self, aggregate: u64, tier: ScopeTier, preamble: String, tools_json: Value) {
        self.inner
            .insert((aggregate, tier), CachedPreamble { preamble, tools_json });
    }
}

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
// JSON Schema → TypeScript type walker
// ────────────────────────────────────────────────────────────────────────────

const MAX_SCHEMA_DEPTH: usize = 10;

/// Recursively convert a JSON Schema value to a TypeScript type string.
///
/// `depth` guards against pathologically recursive schemas; anything deeper
/// than `MAX_SCHEMA_DEPTH` emits `unknown`.
pub fn schema_to_ts(schema: &Value, depth: usize) -> String {
    if depth > MAX_SCHEMA_DEPTH {
        tracing::warn!(
            depth,
            max = MAX_SCHEMA_DEPTH,
            "JSON Schema depth limit exceeded in Code Mode preamble generation, emitting unknown"
        );
        return "unknown".to_string();
    }

    let Some(obj) = schema.as_object() else {
        return "unknown".to_string();
    };

    // anyOf → union
    if let Some(any_of) = obj.get("anyOf").and_then(Value::as_array) {
        let variants: Vec<String> = any_of.iter().map(|v| schema_to_ts(v, depth + 1)).collect();
        return variants.join(" | ");
    }

    // enum → literal union
    if let Some(enum_vals) = obj.get("enum").and_then(Value::as_array) {
        let literals: Vec<String> = enum_vals
            .iter()
            .map(|v| match v {
                Value::String(s) => format!("\"{s}\""),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => "null".to_string(),
                _ => "unknown".to_string(),
            })
            .collect();
        if literals.is_empty() {
            return "unknown".to_string();
        }
        return literals.join(" | ");
    }

    // type-based dispatch
    match obj.get("type").and_then(Value::as_str) {
        Some("string") => "string".to_string(),
        Some("integer") | Some("number") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("null") => "null".to_string(),
        Some("array") => {
            let item_ts = obj.get("items").map_or_else(
                || "unknown".to_string(),
                |items| schema_to_ts(items, depth + 1),
            );
            format!("Array<{item_ts}>")
        }
        Some("object") | None => {
            // object with properties → inline type
            if let Some(props) = obj.get("properties").and_then(Value::as_object) {
                let required: Vec<&str> = obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();

                let mut fields: Vec<String> = props
                    .iter()
                    .map(|(key, val)| {
                        let optional = if required.contains(&key.as_str()) {
                            ""
                        } else {
                            "?"
                        };
                        let ts_type = schema_to_ts(val, depth + 1);
                        // Sanitize key: if it contains special chars, quote it
                        let safe_key = if key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                            key.clone()
                        } else {
                            format!("\"{key}\"")
                        };
                        format!("{safe_key}{optional}: {ts_type}")
                    })
                    .collect();

                // Sort fields for deterministic output
                fields.sort();

                if fields.is_empty() {
                    "Record<string, unknown>".to_string()
                } else {
                    format!("{{ {} }}", fields.join("; "))
                }
            } else {
                "Record<string, unknown>".to_string()
            }
        }
        _ => "unknown".to_string(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JSDoc extraction
// ────────────────────────────────────────────────────────────────────────────

const JSDOC_SUMMARY_MAX: usize = 120;

/// Extract the first sentence from a tool description, truncated to 120 chars.
fn extract_summary(description: &str) -> String {
    let trimmed = description.trim();
    // First sentence ends at `. `, `.\n`, or end of string
    let first = trimmed
        .split_once(". ")
        .map(|(s, _)| s)
        .or_else(|| trimmed.split_once(".\n").map(|(s, _)| s))
        .unwrap_or(trimmed);

    if first.len() > JSDOC_SUMMARY_MAX {
        format!("{}…", &first[..JSDOC_SUMMARY_MAX])
    } else {
        first.to_string()
    }
}

/// Build a JSDoc comment block for a tool function.
fn build_jsdoc(description: &str, schema: Option<&Value>) -> String {
    let summary = extract_summary(description);
    let mut lines: Vec<String> = vec![" * ".to_string() + &summary];

    // Per-param JSDoc from schema properties descriptions
    if let Some(schema_obj) = schema.and_then(Value::as_object) {
        if let Some(props) = schema_obj.get("properties").and_then(Value::as_object) {
            let mut param_keys: Vec<&String> = props.keys().collect();
            param_keys.sort();
            for key in param_keys {
                let prop = &props[key];
                if let Some(desc) = prop
                    .as_object()
                    .and_then(|p| p.get("description"))
                    .and_then(Value::as_str)
                {
                    let truncated = if desc.len() > JSDOC_SUMMARY_MAX {
                        format!("{}…", &desc[..JSDOC_SUMMARY_MAX])
                    } else {
                        desc.to_string()
                    };
                    lines.push(format!(" * @param {key} - {truncated}"));
                }
            }
        }
    }

    let inner = lines.join("\n");
    format!("/**\n{inner}\n */")
}

// ────────────────────────────────────────────────────────────────────────────
// JS proxy generation (runtime executable, not type declarations)
// ────────────────────────────────────────────────────────────────────────────

/// Build a mapping from `"{upstream}::{snake_name}"` → `"{upstream}::{dotted.name}"`.
///
/// Used by the JS proxy so that `codemode.radarr.movie_search(p)` can call
/// `callTool("upstream::radarr::movie.search", p)`.
#[allow(dead_code)]
pub fn build_reverse_snake_map(
    tools: &[UpstreamTool],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for tool in tools {
        let snake = tool_name_to_snake(tool.tool.name.as_ref());
        let upstream = tool.upstream_name.as_ref();
        let dotted = tool.tool.name.as_ref();
        let key = format!("{upstream}::{snake}");
        let value = format!("{upstream}::{dotted}");
        map.insert(key, value);
    }
    map
}

/// Generate a JavaScript preamble string that defines the `codemode` proxy
/// namespace, `__catalog__`, and `__upstreams__` for use inside the sandbox.
///
/// The output is a JS snippet (not TypeScript) that is prepended to user code
/// before being sent to the runner subprocess. It relies on `callTool` already
/// being registered in the sandbox.
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

    // Emit per-upstream namespace objects.
    for (upstream_name, upstream_tools) in &by_upstream {
        // Build snake_case → dotted name mapping, last registration wins on collision.
        let mut snake_to_dotted: BTreeMap<String, String> = BTreeMap::new();
        let mut sorted_tools = upstream_tools.to_vec();
        sorted_tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));
        for tool in &sorted_tools {
            let snake = tool_name_to_snake(tool.tool.name.as_ref());
            snake_to_dotted.insert(snake, tool.tool.name.to_string());
        }

        // Serialize the upstream name safely.
        let upstream_json =
            serde_json::to_string(upstream_name).unwrap_or_else(|_| "\"unknown\"".to_string());

        let mut method_defs = Vec::new();
        for (snake, dotted) in &snake_to_dotted {
            let tool_id = format!("upstream::{upstream_name}::{dotted}");
            let tool_id_json =
                serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"unknown\"".to_string());
            // Always use a JSON-quoted property key so that any residual special
            // characters in the snake_case name (e.g. from exotic tool schemas) never
            // cause a JS syntax error inside QuickJS.
            let snake_json =
                serde_json::to_string(snake.as_str()).unwrap_or_else(|_| format!("\"{snake}\""));
            method_defs.push(format!("    {snake_json}: function(p) {{ return callTool({tool_id_json}, p == null ? {{}} : p); }}"));
        }

        let methods = method_defs.join(",\n");
        let _ = write!(parts, "codemode[{upstream_json}] = {{\n{methods}\n}};\n");
    }

    // Emit __meta__.upstreams value.
    let upstreams_json = serde_json::to_string(upstreams).unwrap_or_else(|_| "[]".to_string());

    format!(
        "// Code Mode preamble — auto-generated\n\
         var codemode = {{}};\n\
         {parts}\
         codemode.__meta__ = {{ upstreams: function() {{ return Promise.resolve({upstreams_json}); }} }};\n\
         var __upstreams__ = {upstreams_json};\n"
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Preamble generation
// ────────────────────────────────────────────────────────────────────────────

/// Build the `declare namespace codemode { ... }` TypeScript string from a
/// slice of upstream tools, including the `callTool` and `__meta__` helper
/// namespaces.
///
/// Tools are grouped by upstream name; within each upstream, tools are
/// sorted for deterministic output.
pub fn generate_preamble(tools: &[UpstreamTool], truncated: bool, dropped_count: usize) -> String {
    use std::collections::BTreeMap;

    // Group tools by upstream, then by camelCase name within each upstream.
    // BTreeMap preserves sorted order for deterministic output.
    let mut by_upstream: BTreeMap<&str, Vec<&UpstreamTool>> = BTreeMap::new();
    for tool in tools {
        by_upstream
            .entry(tool.upstream_name.as_ref())
            .or_default()
            .push(tool);
    }

    let mut upstream_blocks: Vec<String> = Vec::new();

    for (upstream_name, upstream_tools) in &by_upstream {
        // Build snake_case → tool mapping, detecting collisions
        let mut snake_map: BTreeMap<String, &UpstreamTool> = BTreeMap::new();
        let mut sorted_tools = upstream_tools.to_vec();
        sorted_tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));

        for tool in &sorted_tools {
            let snake = tool_name_to_snake(tool.tool.name.as_ref());
            if snake_map.contains_key(&snake) {
                // Note: name collision resolved, last registration wins.
                tracing::debug!(
                    upstream = *upstream_name,
                    tool_name = tool.tool.name.as_ref(),
                    snake_name = %snake,
                    "Code Mode preamble: tool name collision detected, last registration wins"
                );
            }
            snake_map.insert(snake, tool);
        }

        // Build function declarations
        let mut fn_decls: Vec<String> = Vec::new();
        for (snake, tool) in &snake_map {
            let description = tool
                .tool
                .description
                .as_ref()
                .map(|d| d.as_ref())
                .unwrap_or("");

            let jsdoc = build_jsdoc(description, tool.input_schema.as_ref());

            // Build params type from input schema
            let params_type = tool
                .input_schema
                .as_ref()
                .map(|s| schema_to_ts(s, 0))
                .unwrap_or_else(|| "Record<string, unknown>".to_string());

            // Build return type from output schema if upstream advertised one.
            // rmcp `Tool.output_schema: Option<Arc<JsonObject>>` (rmcp 1.6+).
            // Fall back to `unknown` when absent so the contract still parses.
            let return_type = tool
                .tool
                .output_schema
                .as_ref()
                .map(|s| {
                    let value = serde_json::Value::Object((**s).clone());
                    schema_to_ts(&value, 0)
                })
                .unwrap_or_else(|| "unknown".to_string());

            fn_decls.push(format!(
                "    {jsdoc}\n    function {snake}(params: {params_type}): Promise<{return_type}>;"
            ));
        }

        let fn_body = fn_decls.join("\n");
        upstream_blocks.push(format!("  namespace {upstream_name} {{\n{fn_body}\n  }}"));
    }

    // Add built-in callTool escape hatch namespace
    upstream_blocks.push(
        "  namespace callTool {\n    function call<T = unknown>(id: `${string}::${string}::${string}`, params: Record<string, unknown>): Promise<T>;\n  }".to_string(),
    );

    // Add __meta__ namespace
    upstream_blocks.push(
        "  namespace __meta__ {\n    function upstreams(): Promise<string[]>;\n  }".to_string(),
    );

    let namespace_body = upstream_blocks.join("\n");

    // Build __catalog__ declaration
    let catalog_decl = if truncated && dropped_count > 0 {
        format!(
            "declare const __catalog__: string | undefined;\n// Catalog truncated: {dropped_count} tools omitted. Use callTool() escape hatch for unlisted tools."
        )
    } else {
        "declare const __catalog__: undefined;".to_string()
    };

    format!("declare namespace codemode {{\n{namespace_body}\n}}\n{catalog_decl}")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::Tool;
    use serde_json::json;

    use super::*;
    use crate::dispatch::upstream::types::UpstreamTool;

    fn make_upstream_tool(
        upstream: &str,
        name: &str,
        description: Option<&str>,
        schema: Option<Value>,
        destructive: bool,
    ) -> UpstreamTool {
        let tool = Tool::new(
            name.to_string(),
            description.unwrap_or("").to_string(),
            Arc::new(serde_json::Map::new()),
        );
        UpstreamTool {
            tool,
            input_schema: schema,
            upstream_name: Arc::from(upstream),
            destructive,
        }
    }

    // ── Preamble roundtrip ────────────────────────────────────────────────────

    #[test]
    fn typed_preamble_roundtrip_basic_structure() {
        let tools = vec![make_upstream_tool(
            "radarr",
            "movie.search",
            Some("Search for movies"),
            Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "year": {"type": "integer"}
                },
                "required": ["query"]
            })),
            false,
        )];

        let preamble = generate_preamble(&tools, false, 0);

        // PRESENCE: namespace with upstream name exists
        assert!(
            preamble.contains("namespace radarr"),
            "preamble must have radarr namespace, got:\n{preamble}"
        );
        // PRESENCE: snake_case function name for dotted tool name (Cloudflare parity)
        assert!(
            preamble.contains("movie_search"),
            "preamble must have snake_case movie_search, got:\n{preamble}"
        );
        // PRESENCE: typed parameter from schema
        assert!(
            preamble.contains("query: string") || preamble.contains("query"),
            "preamble must have query param"
        );
        // PRESENCE: Promise return type
        assert!(
            preamble.contains("Promise<unknown>"),
            "preamble must have Promise<unknown> return type"
        );
        // PRESENCE: outer namespace wrapper
        assert!(
            preamble.contains("declare namespace codemode"),
            "preamble must wrap in declare namespace codemode"
        );

        // ABSENCE: dotted name must not appear as a function name in TypeScript
        assert!(
            !preamble.contains("function movie.search"),
            "preamble must not have dotted name in TS function"
        );
        // ABSENCE: no markdown fences
        assert!(
            !preamble.contains("```"),
            "preamble must not have markdown fences"
        );
        // ABSENCE: must be namespace, not const declaration for upstream
        assert!(
            !preamble.contains("declare const radarr"),
            "upstream should be namespace, not const"
        );
    }

    #[test]
    fn typed_preamble_optional_params_marked_correctly() {
        let tools = vec![make_upstream_tool(
            "sonarr",
            "series.search",
            Some("Search series"),
            Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "year": {"type": "integer"}
                },
                "required": ["query"]
            })),
            false,
        )];

        let preamble = generate_preamble(&tools, false, 0);

        // PRESENCE: required param has no question mark
        assert!(
            preamble.contains("query: string"),
            "required param must not be optional"
        );
        // PRESENCE: optional param has question mark
        assert!(
            preamble.contains("year?: number"),
            "optional integer param should be year?: number"
        );
        // ABSENCE: required param must not be marked optional
        assert!(
            !preamble.contains("query?: string"),
            "required param must not be optional"
        );
    }

    #[test]
    fn typed_preamble_truncation_notice_when_dropped() {
        let tools = vec![make_upstream_tool(
            "radarr",
            "movie.search",
            Some("Search"),
            None,
            false,
        )];

        let preamble_normal = generate_preamble(&tools, false, 0);
        let preamble_truncated = generate_preamble(&tools, true, 5);

        // PRESENCE: truncated preamble has the note
        assert!(
            preamble_truncated.contains("5 tools omitted"),
            "truncated preamble must mention dropped count"
        );
        // ABSENCE: non-truncated preamble must not have the truncation note
        assert!(
            !preamble_normal.contains("tools omitted"),
            "non-truncated preamble must not have truncation note"
        );
    }

    // ── Preamble cache ────────────────────────────────────────────────────────

    #[test]
    fn preamble_cache_hit_returns_cached_value() {
        let cache = PreambleCache::new();

        // Cold cache: must be None
        assert!(
            cache.get(42, ScopeTier::Admin).is_none(),
            "fresh cache must return None"
        );

        cache.insert(
            42,
            ScopeTier::Admin,
            "declare namespace codemode {}".to_string(),
            serde_json::json!([]),
        );

        // PRESENCE: inserted value is returned
        assert_eq!(
            cache.get(42, ScopeTier::Admin).map(|c| c.preamble),
            Some("declare namespace codemode {}".to_string()),
            "cache must return inserted value"
        );

        // ABSENCE: different tier is a cache miss
        assert!(
            cache.get(42, ScopeTier::Read).is_none(),
            "different scope tier must be cache miss"
        );
        // ABSENCE: different hash is a cache miss
        assert!(
            cache.get(99, ScopeTier::Admin).is_none(),
            "different hash must be cache miss"
        );
        // ABSENCE: execute tier is also a miss if not inserted
        assert!(
            cache.get(42, ScopeTier::Execute).is_none(),
            "Execute tier must be distinct from Admin"
        );
    }

    #[test]
    fn preamble_cache_separate_entries_per_tier() {
        let cache = PreambleCache::new();
        let empty = serde_json::json!([]);
        cache.insert(1, ScopeTier::Admin, "admin-preamble".to_string(), empty.clone());
        cache.insert(1, ScopeTier::Read, "read-preamble".to_string(), empty);

        // PRESENCE: each tier has its own value
        assert_eq!(
            cache.get(1, ScopeTier::Admin).map(|c| c.preamble),
            Some("admin-preamble".to_string())
        );
        assert_eq!(
            cache.get(1, ScopeTier::Read).map(|c| c.preamble),
            Some("read-preamble".to_string())
        );
        // ABSENCE: values are not mixed up
        assert_ne!(
            cache.get(1, ScopeTier::Admin).map(|c| c.preamble),
            cache.get(1, ScopeTier::Read).map(|c| c.preamble),
            "different tiers must return different values"
        );
    }

    // ── Aggregate catalog hash ────────────────────────────────────────────────

    #[test]
    fn aggregate_catalog_hash_is_order_independent() {
        let upstreams_a = vec![
            UpstreamCatalogHash {
                upstream: "radarr".to_string(),
                hash: 1,
            },
            UpstreamCatalogHash {
                upstream: "sonarr".to_string(),
                hash: 2,
            },
        ];
        let upstreams_b = vec![
            UpstreamCatalogHash {
                upstream: "sonarr".to_string(),
                hash: 2,
            },
            UpstreamCatalogHash {
                upstream: "radarr".to_string(),
                hash: 1,
            },
        ];

        // PRESENCE: same set in different order must produce equal hash
        assert_eq!(
            aggregate_catalog_hash(&upstreams_a),
            aggregate_catalog_hash(&upstreams_b),
            "aggregate hash must be order-independent"
        );
    }

    #[test]
    fn aggregate_catalog_hash_changes_when_upstream_changes() {
        let v1 = vec![UpstreamCatalogHash {
            upstream: "radarr".to_string(),
            hash: 1,
        }];
        let v2 = vec![UpstreamCatalogHash {
            upstream: "radarr".to_string(),
            hash: 2,
        }];
        let v3 = vec![UpstreamCatalogHash {
            upstream: "sonarr".to_string(),
            hash: 1,
        }];

        // PRESENCE: different hash value changes the aggregate
        assert_ne!(
            aggregate_catalog_hash(&v1),
            aggregate_catalog_hash(&v2),
            "changing hash must change aggregate"
        );
        // PRESENCE: different upstream name changes the aggregate
        assert_ne!(
            aggregate_catalog_hash(&v1),
            aggregate_catalog_hash(&v3),
            "changing upstream name must change aggregate"
        );
    }

    #[test]
    fn aggregate_catalog_hash_empty_is_stable() {
        // PRESENCE: empty slice returns a deterministic value
        let h1 = aggregate_catalog_hash(&[]);
        let h2 = aggregate_catalog_hash(&[]);
        assert_eq!(h1, h2, "empty aggregate must be deterministic");
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
        assert_eq!(tool_name_to_snake("create/issue"), "create_issue");
        assert_eq!(tool_name_to_snake("list:repos"), "list_repos");
        assert_eq!(tool_name_to_snake("repos/create/branch"), "repos_create_branch");
        // PRESENCE: already-snake input is preserved
        assert_eq!(tool_name_to_snake("create_issue"), "create_issue");
        // PRESENCE: leading digit gets prefixed with underscore
        assert_eq!(tool_name_to_snake("2fa_setup"), "_2fa_setup");
        // ABSENCE: separators must not appear in output (would break JS syntax)
        assert!(!tool_name_to_snake("create/issue").contains('/'));
        assert!(!tool_name_to_snake("list:repos").contains(':'));
    }

    #[test]
    fn generate_js_proxy_quoted_keys_tolerate_special_chars() {
        use crate::dispatch::upstream::types::UpstreamTool;
        use std::sync::Arc;

        // Tool with a slash in the name — previously caused "Exception generated
        // by QuickJS" because the unquoted key was a JS syntax error.
        let schema = Arc::new(
            serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"type": "object", "properties": {}}),
            )
            .unwrap(),
        );
        let tool = UpstreamTool {
            upstream_name: Arc::from("github"),
            tool: rmcp::model::Tool::new("create/issue", "Create an issue", Arc::clone(&schema)),
            destructive: false,
            input_schema: None,
        };
        let js = generate_js_proxy(&[tool], &["github".to_string()]);

        // PRESENCE: the upstream object must be present
        assert!(js.contains("codemode[\"github\"]"), "upstream block missing");
        // PRESENCE: the tool key must appear as a quoted string (not unquoted)
        assert!(js.contains("\"create_issue\""), "snake_case key must be quoted");
        // ABSENCE: no unquoted slash-containing key that would break JS syntax
        assert!(!js.contains("create/issue:"), "slash in unquoted key would break JS");
        // PRESENCE: callTool must be wired to the original dotted tool id
        assert!(js.contains("upstream::github::create/issue"), "original tool id must be preserved");
    }
}
