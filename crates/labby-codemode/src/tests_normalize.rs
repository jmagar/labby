//! Tests: normalize_user_code shapes, caller capability gating, oauth subject.
#![cfg(test)]

#[test]
fn state_and_git_globals_are_present_in_preamble() {
    let js = crate::preamble::generate_local_provider_js();
    assert!(js.contains("function __labLocalProviderCall"));
    assert!(js.contains("return callTool(id, params == null ? {} : params);"));
    assert!(js.contains("globalThis.state"));
    assert!(js.contains("globalThis.git"));
    for method in [
        "state::readFile",
        "state::writeFile",
        "state::list",
        "state::readdir",
        "state::glob",
        "state::searchFiles",
        "state::replaceInFiles",
        "state::planEdits",
        "state::applyEditPlan",
        "git::init",
        "git::status",
        "git::add",
        "git::commit",
        "git::log",
        "git::diff",
    ] {
        assert!(js.contains(method), "{method} missing from preamble");
    }
}

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
    let loose = "const x = await callTool('github::search_issues', {});\nx.items";
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
fn normalize_user_code_wraps_export_default_arrow_with_prologue() {
    // Boa's parse_module cannot parse an arrow in default-export position, so a
    // module with a prologue (`const x = 1; export default async () => x`) used
    // to fall through to loose-wrapping, which left `export default` inside the
    // wrapper body and produced invalid JS. The prologue-aware fallback must turn
    // it into a single valid function expression that runs the prologue and
    // invokes the arrow (which closes over the prologue binding).
    let result = super::normalize_user_code("const x = 1; export default async () => x");
    assert!(
        !result.contains("export default"),
        "export default must be removed: {result}"
    );
    assert!(
        result.starts_with("async () => {"),
        "must be a bare async function expression: {result}"
    );
    assert!(
        result.contains("const x = 1"),
        "prologue must be kept: {result}"
    );
    assert!(
        result.contains("(async () => x)()"),
        "the arrow must be invoked so its result is returned: {result}"
    );
}

#[test]
fn normalize_user_code_wraps_export_default_plain_arrow_with_prologue() {
    // A *plain* (non-async) arrow default export parses fine as a
    // DefaultAssignmentExpression, so unlike the async arrow it goes through the
    // AST path (normalize_module_code), not the textual fallback. The prologue
    // must still be preserved and the arrow invoked.
    let result = super::normalize_user_code("const n = 7;\nexport default () => n;");
    assert!(!result.contains("export default"), "got: {result}");
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(
        result.contains("const n = 7"),
        "prologue must be kept: {result}"
    );
    // Boa reformats the arrow body (`() => n` → `() => { return n; }`), so assert
    // behavior, not exact shape: the prologue runs and the arrow (which returns n)
    // is invoked, not returned uncalled.
    assert!(
        result.contains("return await ("),
        "the entry must be invoked: {result}"
    );
    assert!(
        result.contains("return n"),
        "arrow body must reference n: {result}"
    );
}

#[test]
fn normalize_user_code_does_not_split_export_default_inside_a_string() {
    // A valid script whose string literal merely contains "; export default "
    // must parse as a script (returning the trailing expression `s`), not be
    // split textually into a broken module wrapper. The textual fallback only
    // runs after both parses fail, so this never reaches it.
    let result = super::normalize_user_code("const s = \"; export default \"; s");
    assert!(
        !result.contains("return await ("),
        "a string literal must not be IIFE-wrapped as an export default: {result}"
    );
    assert!(result.contains("const s ="), "prologue kept: {result}");
    assert!(
        result.contains("return (s)"),
        "the trailing expression must be returned: {result}"
    );
}

#[test]
fn normalize_user_code_export_default_arrow_with_trailing_comment_prologue() {
    // A trailing line comment after the prologue's `;` must not defeat the
    // statement-boundary check for the textual arrow-default fallback.
    let result = super::normalize_user_code("const x = 1; // note\nexport default async () => x");
    assert!(!result.contains("export default"), "got: {result}");
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(result.contains("const x = 1"), "got: {result}");
    assert!(result.contains("(async () => x)()"), "got: {result}");
}

#[test]
fn normalize_user_code_export_default_arrow_after_url_string_prologue() {
    // Regression: a `//` inside a prologue string (e.g. a URL) must not corrupt
    // the statement-boundary check. The prologue already ends at `;`, so the
    // textual fallback must split rather than loose-wrap into invalid JS.
    let result = super::normalize_user_code("const u = \"http://x\"; export default async () => u");
    assert!(!result.contains("export default"), "got: {result}");
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(
        result.contains("http://x"),
        "prologue string must be kept: {result}"
    );
    assert!(result.contains("(async () => u)()"), "got: {result}");
}

#[test]
fn normalize_user_code_export_default_arrow_url_string_and_trailing_comment_prologue() {
    // Regression (CodeRabbit): a prologue tail with BOTH a `//` inside a URL
    // string AND a real trailing line comment. `strip_trailing_comment` bails
    // (it sees two `//` and can't tell string from comment), so the suffix-only
    // strip must carry the `;` boundary check and split rather than loose-wrap.
    let result = super::normalize_user_code(
        "const u = \"http://x\"; const x = 1; // note\nexport default async () => x",
    );
    assert!(!result.contains("export default"), "got: {result}");
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(
        result.contains("http://x"),
        "URL string must be kept whole, not corrupted by the strip: {result}"
    );
    assert!(result.contains("const x = 1"), "got: {result}");
    assert!(result.contains("(async () => x)()"), "got: {result}");
}

#[test]
fn normalize_user_code_keeps_named_export_binding_referenced_by_default() {
    // Regression (cubic): a named export the default references must keep its
    // binding in the prologue. Previously every named export was dropped, leaving
    // the default's free variable undefined (ReferenceError) at runtime.
    let result = super::normalize_user_code(
        "export const helper = (n) => n * 2;\nexport default async () => helper(21)",
    );
    assert!(!result.contains("export default"), "got: {result}");
    assert!(
        !result.contains("export const"),
        "the `export` keyword must be stripped from the named export: {result}"
    );
    assert!(
        result.contains("helper"),
        "the named-export binding must be kept in the prologue: {result}"
    );
    assert!(result.starts_with("async () => {"), "got: {result}");
}

#[test]
fn normalize_user_code_keeps_export_var_and_export_function_bindings() {
    // Both binding-carrying named-export forms reach the prologue: `export var`
    // (VarStatement arm) and `export function` (Declaration arm).
    let result = super::normalize_user_code(
        "export var base = 40;\nexport function bump() { return base + 2; }\nexport default async () => bump()",
    );
    assert!(!result.contains("export default"), "got: {result}");
    assert!(
        !result.contains("export var") && !result.contains("export function"),
        "the `export` keyword must be stripped from both named exports: {result}"
    );
    assert!(result.contains("base"), "export var binding kept: {result}");
    assert!(
        result.contains("function bump"),
        "export function binding kept: {result}"
    );
}

#[test]
fn normalize_user_code_drops_import_in_async_arrow_default_prologue() {
    // Regression (cubic, follow-up): a textual-fallback prologue containing only
    // an `import` (no named export) must still be rewritten — the sandbox has no
    // module loader, and a verbatim `import` inside the wrapper is a syntax error.
    // Drop it (matching the module path), leaving at most a runtime ReferenceError
    // rather than a parse failure that kills the whole script.
    let result =
        super::normalize_user_code("import { x } from \"y\";\nexport default async () => 1");
    assert!(
        !result.contains("import"),
        "import must be dropped: {result}"
    );
    assert!(!result.contains("export default"), "got: {result}");
    assert!(result.starts_with("async () => {"), "got: {result}");
}

#[test]
fn normalize_user_code_export_default_class_with_prologue() {
    // The DefaultClassDeclaration AST arm is only reachable with a prologue (a
    // bare class default is caught by the start-anchored strip). Prologue kept.
    let result = super::normalize_user_code("const base = 1;\nexport default class Foo {}");
    assert!(!result.contains("export default"), "got: {result}");
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(
        result.contains("base = 1"),
        "prologue must be kept: {result}"
    );
    assert!(result.contains("class Foo"), "got: {result}");
}

#[test]
fn normalize_user_code_export_default_with_trailing_named_export() {
    // A named export *after* `export default` must not shadow the default in the
    // module-item scan. This needs a prologue so the input does not start with
    // `export default` (which would take the start-anchored strip) and instead
    // goes through normalize_module_code. Pre-fix the scan captured the trailing
    // `export const y` and bailed to an invalid loose-wrap.
    let result = super::normalize_user_code("const x = 1;\nexport default x;\nexport const y = 2;");
    assert!(!result.contains("export default"), "got: {result}");
    assert!(
        !result.contains("export const"),
        "the trailing named export must not leak into the wrapper: {result}"
    );
    assert!(result.starts_with("async () => {"), "got: {result}");
    assert!(
        result.contains("const x = 1"),
        "prologue must be kept: {result}"
    );
}

#[test]
fn normalize_user_code_named_default_function_body_literal_not_corrupted() {
    // wrap_default_fn_as_iife strips only Boa's synthesized leading `default`
    // name. A genuinely-named default export whose body contains the literal
    // `function default(` (here inside a string) must survive verbatim — an
    // unanchored replace would corrupt it to `function(`. Needs a NAMED fn plus a
    // prologue to route through the AST (a bare default takes the start-anchored
    // strip path).
    let result = super::normalize_user_code(
        "const x = 1;\nexport default function named() { return \"function default(\"; }",
    );
    assert!(!result.contains("export default"), "got: {result}");
    assert!(
        result.contains("function named"),
        "named function kept: {result}"
    );
    assert!(
        result.contains("function default("),
        "the string-literal body must survive verbatim, not be corrupted: {result}"
    );
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

#[test]
fn normalize_user_code_does_not_strip_https_url_inside_arrow_body() {
    let source = r#"async () => { return { url: "https://github.com/jmagar/example.git" }; }"#;
    let result = super::normalize_user_code(source);
    assert!(
        result.contains("https://github.com/jmagar/example.git"),
        "URL string must survive normalization, got: {result}"
    );
}

#[test]
fn normalize_user_code_finds_export_default_after_prologue() {
    // A module with prologue statements before `export default` used to fall
    // through to the loose-wrap path (requiring exactly one module item),
    // producing invalid wrapper JS like `return (export default ...)`. The export
    // must now be located among the items, ignoring the prologue. Covers both the
    // DefaultAssignmentExpression arm and the DefaultAsyncFunctionDeclaration arm.
    for source in [
        // DefaultAssignmentExpression with a leading const prologue.
        "const base = 41;\nexport default base + 1;",
        // DefaultAsyncFunctionDeclaration with a leading const prologue.
        "const base = 41;\nexport default async function () { return base + 1; }",
    ] {
        let result = super::normalize_user_code(source);
        assert!(
            !result.contains("export default"),
            "export default must be stripped, got: {result}"
        );
        assert!(
            result.contains("base + 1"),
            "the default-export body must be preserved, got: {result}"
        );
        // Non-vacuous on the PR's core mechanism: the prologue `const base = 41`
        // must survive (otherwise `base` is undefined at runtime). `base + 1`
        // alone lives in the export body and would pass even if the prologue were
        // dropped — this is the assertion that actually guards prologue scoping.
        assert!(
            result.contains("base = 41"),
            "the prologue binding must be preserved, got: {result}"
        );
        assert!(
            result.trim_start().starts_with("async () =>") || result.trim_start().starts_with('('),
            "result must be a wrapped async arrow, got: {result}"
        );
    }
}

#[test]
fn normalize_user_code_strips_trailing_line_comment_after_semicolon() {
    // `; // comment` must not leave a semicolon (or the comment) inside the
    // wrapper grouping `(...)`, which would be a syntax error.
    let result = super::normalize_user_code("async () => 42; // trailing note");
    assert!(result.starts_with("async () =>"), "got: {result}");
    assert!(
        !result.contains("//"),
        "trailing comment must be stripped, got: {result}"
    );
    assert!(
        !result.trim_end().ends_with(';'),
        "trailing semicolon must be stripped, got: {result}"
    );
}

// ── destructive_permitted: caller capability gate ─────────────────────────

fn scoped(scopes: &[&str]) -> super::CodeModeCaller {
    let is_admin = scopes.contains(&"lab:admin");
    super::CodeModeCaller::Scoped {
        capabilities: super::CodeModeCallerCapabilities {
            can_execute: scopes
                .iter()
                .any(|scope| matches!(*scope, "lab" | "lab:admin")),
            can_use_snippets: is_admin,
            is_admin,
        },
        sub: None,
    }
}

#[test]
fn destructive_permitted_for_execute_capable_callers() {
    let surface = super::CodeModeSurface::Mcp;
    assert!(
        super::destructive_permitted(surface, &scoped(&["lab:admin"])),
        "lab:admin caller can execute Code Mode, so do not add a second destructive gate"
    );
    assert!(
        super::destructive_permitted(surface, &scoped(&["lab"])),
        "lab caller can execute Code Mode, so do not add a second destructive gate"
    );
    assert!(
        super::destructive_permitted(super::CodeModeSurface::Cli, &scoped(&["lab:read"])),
        "CLI is trusted local execution and remains permitted"
    );
    assert!(
        super::destructive_permitted(
            super::CodeModeSurface::Mcp,
            &super::CodeModeCaller::TrustedLocal
        ),
        "TrustedLocal MCP callers are already trusted by the host"
    );
}

#[test]
fn destructive_denied_for_read_scope() {
    let surface = super::CodeModeSurface::Mcp;
    assert!(
        !super::destructive_permitted(surface, &scoped(&["lab:read"])),
        "lab:read caller cannot execute Code Mode tools"
    );
}

// ── CodeModeCaller can_execute scope checks ───────────────────────────────

#[test]
fn scoped_caller_can_execute_with_lab_scope() {
    let caller = super::CodeModeCaller::Scoped {
        capabilities: super::CodeModeCallerCapabilities {
            can_execute: true,
            ..super::CodeModeCallerCapabilities::default()
        },
        sub: None,
    };
    assert!(caller.can_execute());
}

#[test]
fn scoped_caller_read_only_cannot_execute() {
    let caller = super::CodeModeCaller::Scoped {
        capabilities: super::CodeModeCallerCapabilities::default(),
        sub: None,
    };
    assert!(
        !caller.can_execute(),
        "lab:read scope must not permit execution"
    );
}

// ── token_estimate_divisor affects truncation (#12b) ─────────────────────
