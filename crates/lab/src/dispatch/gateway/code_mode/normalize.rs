//! User-code normalization for the Code Mode sandbox (Boa parser/interner based).

// Code Mode normalizes user code via the Boa parser/interner (rlib-only). The JS
// engine that actually runs code (both `execute` and `search`) is Javy/QuickJS.
use boa_interner::{Interner, ToIndentedString, ToInternedString};
use boa_parser::{Parser, Source as ParserSource};

/// Normalize user-submitted code before sandbox execution.
///
/// The execute wrapper evaluates `code` as a FUNCTION EXPRESSION
/// (`const __codeModeMain = ({code}); ... return await __codeModeMain();`), so
/// every tolerated input shape must reduce to a *bare parenthesized function
/// expression with no trailing invocation*. A self-invoking IIFE or a trailing
/// `main();` call would break the wrapper (the grouping would contain a Promise
/// or two statements). Transforms:
/// 1. Strip markdown fences (```javascript/typescript/``` wrappers).
/// 2. Bare `function main` / `async function main` declarations → parenthesize
///    into an expression `(async function main() {...})` — NO trailing `main();`.
/// 3. `export default [async] function` → strip `export default ` and
///    parenthesize the function expression — NO trailing IIFE `()`.
/// 4. A bare arrow `async () => {...}` passes through unchanged (it is already a
///    function expression).
/// 5. Loose statements / trailing expressions are wrapped in `async () => { ... }`;
///    if the trailing statement looks like an expression, it is returned.
/// 6. `export default <X>` preceded by prologue statements
///    (`const x = 1; export default async () => x`) keeps the prologue and
///    invokes the default-export entry so it closes over those bindings —
///    handled via the AST when Boa can parse it, and via a textual fallback for
///    the one form it cannot (an arrow in default-export position).
///
/// Only `execute` normalizes the caller's code through this before handing it to
/// the Javy runner. `search` passes its code to the runner *raw* (no
/// normalization) so that a non-function search input still surfaces as a
/// contract error instead of being silently wrapped into a valid async arrow.
///
/// Exposed (`pub`) so integration tests can normalize a body form and pipe the
/// exact post-normalize string through the runner end to end.
pub fn normalize_user_code(code: &str) -> String {
    let code = strip_code_fences(code.trim()).trim();
    if code.is_empty() {
        return "async () => {}".to_string();
    }
    if let Some(inner) = code.strip_prefix("export default ") {
        let inner = inner.trim().trim_end_matches(';').trim();
        if inner.starts_with("async function") || inner.starts_with("function") {
            return format!("async () => {{\nreturn ({inner})();\n}}");
        }
        if inner.starts_with("class") {
            return format!("async () => {{\nreturn ({inner});\n}}");
        }
        return normalize_user_code(inner);
    }
    if let Some(normalized) = normalize_user_code_parsed(code) {
        return normalized;
    }
    // Reached only when the code parses as neither a module nor a script — i.e.
    // an *arrow* function in `export default` position after a prologue
    // (`const x = 1; export default async () => x`): Boa's parse_module cannot
    // parse an arrow default export, and `export` is invalid in a script, so both
    // parses above returned `None`. Recover textually here. This is safe against
    // false positives: valid script code that merely contains the literal
    // "; export default " (e.g. inside a string) parses as a script above and
    // never reaches this point. Run the prologue first and invoke the no-prologue
    // entry inside one wrapper so it closes over the prologue's bindings.
    if let Some((prologue, value)) = split_prologue_export_default(code) {
        let value = value.trim().trim_end_matches(';').trim();
        if !value.is_empty() {
            let entry = normalize_user_code(&format!("export default {value}"));
            return format!("async () => {{\n{prologue}\nreturn await ({entry})();\n}}");
        }
    }
    wrap_loose_code_as_async_arrow(code)
}

/// Split `{prologue} export default {value}` into the prologue and the
/// default-export value, but only when `export default` appears at a statement
/// boundary after a non-empty prologue (the prologue ends at a `;` or `}`).
///
/// This is a conservative textual fallback used only after the AST-based
/// normalizers fail — which happens for an arrow function in default-export
/// position, since Boa's `parse_module` cannot parse that form. The boundary
/// check keeps it from firing on an `export default` substring inside an
/// expression. The start-anchored `export default` case is handled earlier in
/// `normalize_user_code`, so this only fires when a real prologue is present.
fn split_prologue_export_default(code: &str) -> Option<(&str, &str)> {
    const NEEDLE: &str = "export default ";
    let mut from = 0;
    while let Some(rel) = code[from..].find(NEEDLE) {
        let idx = from + rel;
        let before = code[..idx].trim_end();
        // Skip a trailing line/block comment when checking the statement boundary
        // so `const x = 1; // note\nexport default ...` still splits. The comment
        // is kept in the returned prologue (harmless — it precedes the `return`).
        let boundary = strip_trailing_comment(before).trim_end();
        if !boundary.is_empty() && (boundary.ends_with(';') || boundary.ends_with('}')) {
            return Some((before, &code[idx + NEEDLE.len()..]));
        }
        from = idx + NEEDLE.len();
    }
    None
}

fn wrap_loose_code_as_async_arrow(code: &str) -> String {
    let code = code.trim().trim_end_matches(';').trim();
    if code.is_empty() {
        return "async () => {}".to_string();
    }
    if let Some((before, after)) = code.rsplit_once(';') {
        let trailing = after.trim();
        if !trailing.is_empty() && looks_like_returnable_expression(trailing) {
            return format!("async () => {{\n{before};\nreturn ({trailing})\n}}");
        }
    } else if looks_like_returnable_expression(code) && !code.trim_start().starts_with("return ") {
        return format!("async () => {{\nreturn ({code})\n}}");
    }

    format!("async () => {{\n{code}\n}}")
}

fn strip_code_fences(code: &str) -> &str {
    let trimmed = code.trim();
    for lang in ["javascript", "typescript", "tsx", "jsx", "js", "ts", ""] {
        let prefix = if lang.is_empty() {
            "```\n".to_string()
        } else {
            format!("```{lang}\n")
        };
        if let Some(stripped) = trimmed.strip_prefix(&prefix)
            && let Some(inner) = stripped.strip_suffix("```")
        {
            return inner.trim();
        }
    }
    trimmed
}

fn normalize_user_code_parsed(source: &str) -> Option<String> {
    normalize_module_code(source).or_else(|| normalize_script_code(source))
}

fn normalize_module_code(source: &str) -> Option<String> {
    let mut interner = Interner::default();
    let mut parser = Parser::new(ParserSource::from_bytes(source.as_bytes()));
    let module = parser
        .parse_module(&boa_ast::scope::Scope::new_global(), &mut interner)
        .ok()?;
    // Separate the `export default` declaration from any prologue statements, so a
    // module with leading statements (`const x = 1; export default <X>`) keeps
    // those bindings. Rendering only the export left the default's free variables
    // undefined at runtime. Imports are skipped — the sandbox has no module loader.
    let mut prologue: Vec<String> = Vec::new();
    let mut export = None;
    for item in module.items().items() {
        match item {
            boa_ast::ModuleItem::ExportDeclaration(decl) => export = Some(decl),
            boa_ast::ModuleItem::ImportDeclaration(_) => {}
            boa_ast::ModuleItem::StatementListItem(stmt) => {
                prologue.push(stmt.to_indented_string(&interner, 0));
            }
        }
    }
    let entry = match export?.as_ref() {
        boa_ast::declaration::ExportDeclaration::DefaultAssignmentExpression(expr) => {
            normalize_user_code(&expr.to_interned_string(&interner))
        }
        boa_ast::declaration::ExportDeclaration::DefaultFunctionDeclaration(function) => {
            wrap_default_fn_as_iife(&function.to_indented_string(&interner, 0))
        }
        boa_ast::declaration::ExportDeclaration::DefaultAsyncFunctionDeclaration(function) => {
            wrap_default_fn_as_iife(&function.to_indented_string(&interner, 0))
        }
        boa_ast::declaration::ExportDeclaration::DefaultClassDeclaration(class) => format!(
            "async () => {{\nreturn ({});\n}}",
            class.to_indented_string(&interner, 0)
        ),
        _ => return None,
    };
    if prologue.is_empty() {
        Some(entry)
    } else {
        let prologue = prologue.join("\n");
        Some(format!(
            "async () => {{\n{prologue}\nreturn await ({entry})();\n}}"
        ))
    }
}

/// Wrap a rendered `export default` function declaration as an immediately
/// invoked expression inside an async arrow wrapper.
fn wrap_default_fn_as_iife(rendered: &str) -> String {
    format!("async () => {{\nreturn ({rendered})();\n}}")
}

fn normalize_script_code(source: &str) -> Option<String> {
    let mut interner = Interner::default();
    let mut parser = Parser::new(ParserSource::from_bytes(source.as_bytes()));
    let script = parser
        .parse_script(&boa_ast::scope::Scope::new_global(), &mut interner)
        .ok()?;
    let statements = script.statements().statements();
    if statements.is_empty() {
        return Some("async () => {}".to_string());
    }

    if let [item] = statements {
        match item {
            boa_ast::StatementListItem::Statement(statement) => {
                if let boa_ast::Statement::Expression(expr) = statement.as_ref() {
                    if matches!(
                        expr.flatten(),
                        boa_ast::Expression::ArrowFunction(_)
                            | boa_ast::Expression::AsyncArrowFunction(_)
                    ) {
                        return Some(strip_trailing_statement_semicolon(source));
                    }
                    return Some(format!(
                        "async () => {{\nreturn ({})\n}}",
                        expr.to_interned_string(&interner)
                    ));
                }
            }
            boa_ast::StatementListItem::Declaration(declaration) => {
                if let Some(name) = function_declaration_name(declaration.as_ref(), &interner) {
                    return Some(format!(
                        "async () => {{\n{}\nreturn {name}();\n}}",
                        declaration.to_indented_string(&interner, 0)
                    ));
                }
            }
        }
    }

    if let Some((last, before)) = statements.split_last()
        && let boa_ast::StatementListItem::Statement(statement) = last
        && let boa_ast::Statement::Expression(expr) = statement.as_ref()
    {
        let before = before
            .iter()
            .map(|item| item.to_indented_string(&interner, 0))
            .collect::<Vec<_>>()
            .join("\n");
        let expr = expr.to_interned_string(&interner);
        return Some(if before.trim().is_empty() {
            format!("async () => {{\nreturn ({expr})\n}}")
        } else {
            format!("async () => {{\n{before}\nreturn ({expr})\n}}")
        });
    }

    let body = statements
        .iter()
        .map(|item| item.to_indented_string(&interner, 0))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("async () => {{\n{body}\n}}"))
}

fn strip_trailing_statement_semicolon(source: &str) -> String {
    // Strip a trailing line/block comment first so `async () => 42; // note`
    // does not leave a `;` (and the comment) inside the wrapper grouping, which
    // would be a syntax error. After removing any trailing comment + whitespace,
    // drop the statement-terminating semicolon.
    let trimmed = strip_trailing_comment(source.trim_end()).trim_end();
    trimmed.strip_suffix(';').map_or_else(
        || trimmed.to_string(),
        |without| without.trim_end().to_string(),
    )
}

/// Remove a single trailing `// ...` line comment or `/* ... */` block comment
/// (and only when it is genuinely at the end of the source). Conservative: bails
/// out unchanged if a `//` or `*/` also appears earlier, since a mid-source
/// occurrence may be inside a string literal we must not disturb.
fn strip_trailing_comment(source: &str) -> &str {
    let trimmed = source.trim_end();
    if let Some(idx) = trimmed.rfind("//") {
        // Only treat as a trailing line comment if nothing but the comment text
        // follows on the last line (no earlier `//` that could be in a string).
        if trimmed.matches("//").count() == 1 && !trimmed[idx..].contains('\n') {
            return trimmed[..idx].trim_end();
        }
    }
    if trimmed.ends_with("*/")
        && let Some(start) = trimmed.rfind("/*")
        && trimmed.matches("/*").count() == 1
    {
        return trimmed[..start].trim_end();
    }
    trimmed
}

fn function_declaration_name(
    declaration: &boa_ast::Declaration,
    interner: &Interner,
) -> Option<String> {
    match declaration {
        boa_ast::Declaration::FunctionDeclaration(function) => {
            Some(function.name().to_interned_string(interner))
        }
        boa_ast::Declaration::AsyncFunctionDeclaration(function) => {
            Some(function.name().to_interned_string(interner))
        }
        boa_ast::Declaration::GeneratorDeclaration(function) => {
            Some(function.name().to_interned_string(interner))
        }
        boa_ast::Declaration::AsyncGeneratorDeclaration(function) => {
            Some(function.name().to_interned_string(interner))
        }
        _ => None,
    }
}

fn looks_like_returnable_expression(statement: &str) -> bool {
    let statement = statement.trim();
    !statement.is_empty()
        && !matches!(
            statement.split_whitespace().next(),
            Some(
                "const"
                    | "let"
                    | "var"
                    | "return"
                    | "if"
                    | "for"
                    | "while"
                    | "switch"
                    | "try"
                    | "catch"
                    | "function"
                    | "class"
                    | "throw"
            )
        )
}
