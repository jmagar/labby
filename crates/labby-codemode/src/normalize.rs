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
///    the one form it cannot (an *async* arrow in default-export position; a
///    plain arrow parses as a DefaultAssignmentExpression and uses the AST).
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
    // an *async arrow* function in `export default` position after a prologue
    // (`const x = 1; export default async () => x`): Boa's parse_module cannot
    // parse an *async* arrow default export (a plain arrow parses fine as a
    // DefaultAssignmentExpression and takes the AST path), and `export` is invalid
    // in a script, so both parses above returned `None`. Recover textually here.
    // This is safe against
    // false positives: valid script code that merely contains the literal
    // "; export default " (e.g. inside a string) parses as a script above and
    // never reaches this point. Run the prologue first and invoke the no-prologue
    // entry inside one wrapper so it closes over the prologue's bindings.
    if let Some((prologue, value)) = split_prologue_export_default(code) {
        let value = value.trim().trim_end_matches(';').trim();
        if !value.is_empty() {
            // The prologue is kept verbatim except for named exports, whose
            // `export` keyword would be a syntax error inside the wrapper.
            let prologue = strip_prologue_exports(prologue);
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
/// This is a conservative textual fallback. String-literal safety does NOT come
/// from this function — it comes from the caller: this runs only after both the
/// module and script parses fail, so any valid script that merely contains
/// `; export default` inside a string parses as a script first and never reaches
/// here (see `normalize_user_code` and the `..._inside_a_string` test). The
/// start-anchored `export default` case is also handled earlier, so this only
/// fires when a real prologue precedes an otherwise-unparseable arrow default.
fn split_prologue_export_default(code: &str) -> Option<(&str, &str)> {
    const NEEDLE: &str = "export default ";
    let mut from = 0;
    while let Some(rel) = code[from..].find(NEEDLE) {
        let idx = from + rel;
        let before = code[..idx].trim_end();
        // A real statement terminator wins as-is. Only when `before` does not
        // already end at a boundary do we retry after stripping a trailing
        // comment, so `const x = 1; // note\nexport default ...` still splits —
        // without letting a `//` *inside* a prologue string (e.g. a "http://"
        // URL) corrupt an otherwise-terminated prologue.
        let ends_at_boundary = |s: &str| s.ends_with(';') || s.ends_with('}');
        // `strip_trailing_comment` bails on any earlier `//` (it can't tell a
        // string-internal `//` from a comment). That defeats a prologue whose
        // tail holds both a "http://" URL string and a real trailing comment.
        // `strip_suffix_line_comment` inspects only the last `//` on the final
        // line, so it strips the genuine trailing comment regardless. A spurious
        // strip can only split here if the text before that `//` already ends at
        // a `;`/`}` — gated by `ends_at_boundary` and by this running only after
        // both real parses failed.
        let comment_stripped = strip_trailing_comment(before).trim_end();
        let suffix_stripped = strip_suffix_line_comment(before);
        if (!before.is_empty() && ends_at_boundary(before))
            || (!comment_stripped.is_empty() && ends_at_boundary(comment_stripped))
            || (!suffix_stripped.is_empty() && ends_at_boundary(suffix_stripped))
        {
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

/// Whether an export declaration is an `export default ...` form (vs a named
/// export, `export { ... }`, or a re-export).
fn is_default_export(decl: &boa_ast::declaration::ExportDeclaration) -> bool {
    use boa_ast::declaration::ExportDeclaration as E;
    matches!(
        decl,
        E::DefaultFunctionDeclaration(_)
            | E::DefaultGeneratorDeclaration(_)
            | E::DefaultAsyncFunctionDeclaration(_)
            | E::DefaultAsyncGeneratorDeclaration(_)
            | E::DefaultClassDeclaration(_)
            | E::DefaultAssignmentExpression(_)
    )
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
        // Capture the `export default` declaration specifically — not just any
        // export. A *named* export (`export const y = 1`, `export { ... }`, a
        // re-export) must not shadow the default and cause normalization to bail.
        if let boa_ast::ModuleItem::ExportDeclaration(decl) = item
            && is_default_export(decl)
        {
            export = Some(decl);
            continue;
        }
        // Everything else: statements and binding-carrying named exports become
        // prologue (with `export` stripped); imports / `export {}` lists / re-exports
        // have no sandbox-runtime role and render to nothing.
        if let Some(rendered) = render_prologue_item(item, &interner) {
            prologue.push(rendered);
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

/// Render a non-default module item as prologue source, with any `export` keyword
/// stripped. Returns `None` for items with no sandbox-runtime role: `export default`
/// (handled by the caller), `export { a, b }` lists, re-exports, and imports (the
/// sandbox has no module loader). Binding-carrying named exports (`export const`,
/// `export var`, `export function`, `export class`) keep their binding so a default
/// that references them resolves at runtime.
fn render_prologue_item(item: &boa_ast::ModuleItem, interner: &Interner) -> Option<String> {
    match item {
        boa_ast::ModuleItem::StatementListItem(stmt) => Some(stmt.to_indented_string(interner, 0)),
        boa_ast::ModuleItem::ExportDeclaration(decl) => match decl.as_ref() {
            boa_ast::declaration::ExportDeclaration::VarStatement(var) => {
                Some(var.to_interned_string(interner))
            }
            boa_ast::declaration::ExportDeclaration::Declaration(d) => {
                Some(d.to_indented_string(interner, 0))
            }
            _ => None,
        },
        boa_ast::ModuleItem::ImportDeclaration(_) => None,
    }
}

/// Strip `export` keywords from a textual-fallback prologue by reparsing it.
///
/// The async-arrow `export default` fallback keeps the prologue verbatim, but a
/// prologue may carry named exports (`export const helper = ...`) whose `export`
/// keyword is a syntax error inside the generated `async () => { ... }` wrapper.
/// Unlike the full input, the prologue alone has no async-arrow default and so
/// parses cleanly as a module — reparse it and re-render statements and
/// binding-carrying named exports without `export`. Returns the prologue unchanged
/// if it does not parse as a module (e.g. a plain script prologue), preserving the
/// prior behavior for the common no-named-export case.
fn strip_prologue_exports(prologue: &str) -> String {
    let mut interner = Interner::default();
    let mut parser = Parser::new(ParserSource::from_bytes(prologue.as_bytes()));
    let Ok(module) = parser.parse_module(&boa_ast::scope::Scope::new_global(), &mut interner)
    else {
        return prologue.to_string();
    };
    // Only rewrite when the prologue carries a non-statement item — a named
    // export whose `export` keyword must be stripped, or an `import` that must be
    // dropped (no loader in the sandbox; left verbatim it is a syntax error inside
    // the wrapper). A pure-statement prologue is kept verbatim so re-rendering
    // never perturbs the common case.
    let needs_rewrite = module
        .items()
        .items()
        .iter()
        .any(|item| !matches!(item, boa_ast::ModuleItem::StatementListItem(_)));
    if !needs_rewrite {
        return prologue.to_string();
    }
    module
        .items()
        .items()
        .iter()
        .filter_map(|item| render_prologue_item(item, &interner))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wrap a rendered `export default` function declaration as an immediately
/// invoked expression inside an async arrow wrapper.
///
/// Boa renders an *anonymous* `export default [async] function() {...}` with the
/// synthesized name `default` (`async function default() {...}`). `default` is a
/// reserved word, so that is a syntax error when used as an expression in the
/// IIFE. Drop the synthesized name to recover a valid anonymous function
/// expression. (A genuinely named `export default function foo() {}` keeps `foo`
/// and is untouched.)
fn wrap_default_fn_as_iife(rendered: &str) -> String {
    let rendered = strip_synthesized_default_name(rendered);
    format!("async () => {{\nreturn ({rendered})();\n}}")
}

/// Remove Boa's synthesized `default` name from an anonymous default-export
/// function (`[async] function default(...)`), anchored at the leading function
/// keyword. Anchoring matters: an unanchored replace would also rewrite a
/// `function default(` substring appearing *inside* the body (e.g. in a string
/// literal of a genuinely-named default export), silently corrupting it.
fn strip_synthesized_default_name(rendered: &str) -> String {
    for (synthesized, anonymous) in [
        ("async function default(", "async function("),
        ("async function default (", "async function ("),
        ("function default(", "function("),
        ("function default (", "function ("),
    ] {
        if let Some(rest) = rendered.strip_prefix(synthesized) {
            return format!("{anonymous}{rest}");
        }
    }
    rendered.to_string()
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

/// Strip a trailing `// ...` line comment, inspecting only the last `//` on the
/// final line. Unlike [`strip_trailing_comment`], this tolerates an earlier `//`
/// elsewhere in `source` (e.g. inside a `"http://..."` URL string in a prologue).
///
/// Only [`split_prologue_export_default`] uses this, where the result feeds a
/// `;`/`}` boundary check — a string-internal last `//` produces a non-boundary
/// remainder and is rejected there, so this loose strip is safe in that context
/// but is NOT a general-purpose comment stripper.
fn strip_suffix_line_comment(source: &str) -> &str {
    let trimmed = source.trim_end();
    let line_start = trimmed.rfind('\n').map_or(0, |i| i + 1);
    match trimmed[line_start..].rfind("//") {
        Some(rel) => trimmed[..line_start + rel].trim_end(),
        None => trimmed,
    }
}

/// Remove a single trailing `// ...` line comment or `/* ... */` block comment
/// (and only when it is genuinely at the end of the source). Conservative: bails
/// out unchanged if a `//` or `*/` also appears earlier, since a mid-source
/// occurrence may be inside a string literal we must not disturb.
fn strip_trailing_comment(source: &str) -> &str {
    let trimmed = source.trim_end();
    if let Some(start) = trailing_comment_start(trimmed) {
        return trimmed[..start].trim_end();
    }
    trimmed
}

fn trailing_comment_start(source: &str) -> Option<usize> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment(usize),
        BlockComment(usize),
    }

    let mut state = State::Code;
    let mut escaped = false;
    let mut iter = source.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        match state {
            State::Code => match ch {
                '\'' => state = State::Single,
                '"' => state = State::Double,
                '`' => state = State::Template,
                '/' if iter.peek().is_some_and(|(_, next)| *next == '/') => {
                    iter.next();
                    state = State::LineComment(idx);
                }
                '/' if iter.peek().is_some_and(|(_, next)| *next == '*') => {
                    iter.next();
                    state = State::BlockComment(idx);
                }
                _ => {}
            },
            State::Single => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' {
                    state = State::Code;
                }
            }
            State::Double => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    state = State::Code;
                }
            }
            State::Template => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '`' {
                    state = State::Code;
                }
            }
            State::LineComment(start) => {
                if ch == '\n' {
                    state = State::Code;
                } else if iter.peek().is_none() {
                    return Some(start);
                }
            }
            State::BlockComment(start) => {
                if ch == '*' && iter.peek().is_some_and(|(_, next)| *next == '/') {
                    iter.next();
                    if iter.peek().is_none() {
                        return Some(start);
                    }
                    state = State::Code;
                }
            }
        }
    }

    match state {
        State::LineComment(start) => Some(start),
        _ => None,
    }
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
