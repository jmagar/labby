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
///
/// Both `execute` and `search` normalize the caller's code through this before
/// handing it to the Javy runner.
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
    normalize_user_code_parsed(code).unwrap_or_else(|| wrap_loose_code_as_async_arrow(code))
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
    let items = module.items().items();
    let [item] = items else {
        return None;
    };
    let boa_ast::ModuleItem::ExportDeclaration(export) = item else {
        return None;
    };
    match export.as_ref() {
        boa_ast::declaration::ExportDeclaration::DefaultAssignmentExpression(expr) => {
            Some(normalize_user_code(&expr.to_interned_string(&interner)))
        }
        boa_ast::declaration::ExportDeclaration::DefaultFunctionDeclaration(function) => Some(
            wrap_default_fn_as_iife(&function.to_indented_string(&interner, 0)),
        ),
        boa_ast::declaration::ExportDeclaration::DefaultAsyncFunctionDeclaration(function) => Some(
            wrap_default_fn_as_iife(&function.to_indented_string(&interner, 0)),
        ),
        boa_ast::declaration::ExportDeclaration::DefaultClassDeclaration(class) => Some(format!(
            "async () => {{\nreturn ({});\n}}",
            class.to_indented_string(&interner, 0)
        )),
        _ => None,
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
    let trimmed = source.trim();
    trimmed.strip_suffix(';').map_or_else(
        || source.to_string(),
        |without| without.trim_end().to_string(),
    )
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
