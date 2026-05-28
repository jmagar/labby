//! Output formatting for CLI commands.
//!
//! All CLI handlers should call [`print`] with their data. It chooses
//! human-readable or JSON output and keeps the styling logic in one place.

#![allow(clippy::print_stdout)]

use std::fmt::Write as _;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use super::theme::{CliTheme, OutputFormat, OutputKind, RenderContext};

/// Print a serializable value in the requested format.
///
/// `Json` emits compact single-line JSON (machine-readable).
/// `Human` emits a structured terminal view with color and icons when
/// stdout supports ANSI.
pub fn print<T: Serialize>(value: &T, format: OutputFormat) -> Result<()> {
    println!("{}", render(value, format)?);
    Ok(())
}

/// Render a serializable value to a string in the requested format.
///
/// Used by [`print`] and available for testing without stdout capture.
pub fn render<T: Serialize>(value: &T, format: OutputFormat) -> Result<String> {
    Ok(match format.kind() {
        OutputKind::Human => {
            let theme = CliTheme::from_context(format.render_context());
            render_human(&serde_json::to_value(value)?, theme)
        }
        OutputKind::Json => serde_json::to_string(value)?,
    })
}

fn render_human(value: &Value, theme: CliTheme) -> String {
    match value {
        Value::Object(map) => render_object(map, theme, 0),
        Value::Array(items) if items.iter().all(Value::is_object) => {
            render_record_array(items, theme, None)
        }
        Value::Array(items) => render_array(items, theme, 0),
        _ => render_scalar(value, theme),
    }
}

fn render_object(map: &serde_json::Map<String, Value>, theme: CliTheme, indent: usize) -> String {
    if is_doctor_report(map) {
        return render_doctor_report(map, theme, indent);
    }
    if is_catalog(map) {
        return render_catalog(map, theme);
    }

    if map.is_empty() {
        return format!("{}{}", indent_str(indent), theme.muted("∅"));
    }

    let mut lines = Vec::new();
    for (k, v) in map {
        let prefix = indent_str(indent);
        let key = theme.key(k);
        match v {
            Value::Array(items) if items.iter().all(Value::is_object) => {
                lines.push(format!("{}{} {}", prefix, theme.accent("▸"), key));
                lines.push(render_record_array(items, theme, Some(k)));
            }
            Value::Array(items) => {
                lines.push(format!("{}{} {}", prefix, theme.accent("▸"), key));
                lines.push(render_array(items, theme, indent + 1));
            }
            Value::Object(child) => {
                lines.push(format!("{}{} {}", prefix, theme.accent("▸"), key));
                lines.push(render_object(child, theme, indent + 1));
            }
            _ => lines.push(format!(
                "{}{} {} {} {}",
                prefix,
                theme.accent("•"),
                key,
                theme.muted(":"),
                render_scalar(v, theme)
            )),
        }
    }
    lines.join("\n")
}

fn render_record_array(items: &[Value], theme: CliTheme, field_name: Option<&str>) -> String {
    if items.is_empty() {
        return theme.muted("∅");
    }

    if items.iter().all(is_health_row) {
        return render_health_rows(items, theme);
    }
    if items.iter().all(is_doctor_finding) {
        return render_finding_rows(items, theme);
    }
    let headers = collect_headers(items);
    let rows = items
        .iter()
        .filter_map(Value::as_object)
        .map(|map| {
            headers
                .iter()
                .map(|header| {
                    map.get(header)
                        .map_or_else(String::new, |value| render_table_cell(header, value, theme))
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let mut out = String::new();
    if let Some(field_name) = field_name {
        let title = match field_name {
            "creds" => "Credentials",
            "warnings" => "Warnings",
            "findings" => "Findings",
            other => other,
        };
        writeln!(out, "{}", theme.heading(title)).ok();
    }
    out.push_str(&render_table(&headers, &rows, theme));
    out
}

fn render_table(headers: &[String], rows: &[Vec<String>], theme: CliTheme) -> String {
    if headers.is_empty() {
        return theme.muted("[]");
    }

    let mut widths: Vec<usize> = headers.iter().map(|h| visible_width(h)).collect();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            if idx < widths.len() {
                widths[idx] = widths[idx].max(visible_width(cell));
            }
        }
    }

    let mut out = String::new();
    out.push_str(&render_table_row(headers, &widths, |header| {
        theme.section(header)
    }));
    out.push('\n');
    out.push_str(&render_table_separator(&widths, theme));

    for row in rows {
        out.push('\n');
        out.push_str(&render_table_row(row, &widths, Clone::clone));
    }

    out
}

fn render_table_row<F>(cells: &[String], widths: &[usize], map: F) -> String
where
    F: Fn(&String) -> String,
{
    let mut row = String::new();
    for (idx, cell) in cells.iter().enumerate() {
        if idx > 0 {
            row.push(' ');
        }
        let rendered = map(cell);
        let width = widths
            .get(idx)
            .copied()
            .unwrap_or_else(|| visible_width(&rendered));
        row.push_str(&pad_right(&rendered, width));
    }
    row
}

fn render_table_separator(widths: &[usize], theme: CliTheme) -> String {
    let mut line = String::new();
    for (idx, width) in widths.iter().enumerate() {
        if idx > 0 {
            line.push(' ');
        }
        line.push_str(&theme.border(&"─".repeat(*width)));
    }
    line
}

fn render_health_rows(items: &[Value], theme: CliTheme) -> String {
    let mut ok = 0usize;
    let mut warn = 0usize;
    let mut fail = 0usize;
    let mut rows = Vec::new();

    for item in items.iter().filter_map(Value::as_object) {
        let reachable = item
            .get("reachable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let auth_ok = item
            .get("auth_ok")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let status = match (reachable, auth_ok) {
            (true, true) => {
                ok += 1;
                theme.ok_badge()
            }
            (true, false) => {
                warn += 1;
                theme.warn_badge()
            }
            _ => {
                fail += 1;
                theme.error_badge()
            }
        };

        rows.push(vec![
            status,
            item.get("service")
                .map_or_else(|| "-".to_string(), |v| render_cell_text(v, theme)),
            theme.bool_icon(auth_ok),
            item.get("version")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| theme.muted("-")),
            render_latency(item.get("latency_ms"), theme.context()),
            item.get("message")
                .map_or_else(|| theme.muted("-"), |v| render_cell_text(v, theme)),
        ]);
    }

    let headers = vec![
        "Status".to_string(),
        "Service".to_string(),
        "Auth".to_string(),
        "Version".to_string(),
        "Latency".to_string(),
        "Message".to_string(),
    ];

    let mut out = String::new();
    writeln!(
        out,
        "{} {}",
        theme.section("Service Health"),
        theme.muted(format!("({} total)", items.len()).as_str())
    )
    .ok();
    writeln!(
        out,
        "{} {} {} {} {} {} {}",
        theme.primary("Status:"),
        theme.ok_badge(),
        theme.value(ok.to_string().as_str()),
        theme.warn_badge(),
        theme.value(warn.to_string().as_str()),
        theme.error_badge(),
        theme.value(fail.to_string().as_str())
    )
    .ok();
    out.push('\n');
    out.push_str(&render_table(&headers, &rows, theme));
    out
}

fn render_finding_rows(items: &[Value], theme: CliTheme) -> String {
    let mut rows = Vec::new();

    for item in items.iter().filter_map(Value::as_object) {
        let severity = item
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let status = match severity {
            "ok" => theme.ok_badge(),
            "warn" => theme.warn_badge(),
            "fail" => theme.error_badge(),
            _ => theme.muted("?"),
        };

        rows.push(vec![
            status,
            item.get("service")
                .map_or_else(|| "-".to_string(), |v| render_cell_text(v, theme)),
            item.get("check")
                .map_or_else(|| "-".to_string(), |v| render_cell_text(v, theme)),
        ]);
    }

    let headers = vec![
        "Status".to_string(),
        "Service".to_string(),
        "Check".to_string(),
    ];

    render_table(&headers, &rows, theme)
}

fn render_array(items: &[Value], theme: CliTheme, indent: usize) -> String {
    let prefix = indent_str(indent);
    if items.is_empty() {
        return format!("{prefix}{}", theme.muted("∅"));
    }

    if items.iter().all(is_scalar) && items.len() <= 8 {
        let parts: Vec<String> = items.iter().map(|v| render_scalar(v, theme)).collect();
        return format!(
            "{prefix}[{}]",
            parts.join(format!(" {} ", theme.accent("·")).as_str())
        );
    }

    let mut lines = Vec::new();
    for item in items {
        match item {
            Value::Object(map) => {
                lines.push(format!("{prefix}{}", theme.accent("•")));
                lines.push(render_object(map, theme, indent + 1));
            }
            Value::Array(child) => {
                lines.push(format!("{prefix}{}", theme.accent("•")));
                lines.push(render_array(child, theme, indent + 1));
            }
            _ => lines.push(format!(
                "{prefix}{} {}",
                theme.accent("•"),
                render_scalar(item, theme)
            )),
        }
    }
    lines.join("\n")
}

fn render_scalar(value: &Value, theme: CliTheme) -> String {
    match value {
        Value::Null => theme.muted("∅"),
        Value::Bool(true) => theme.ok_badge(),
        Value::Bool(false) => theme.error_badge(),
        Value::Number(n) => theme.value(&n.to_string()),
        Value::String(s) => theme.value(s),
        other => other.to_string(),
    }
}

fn render_cell_text(value: &Value, theme: CliTheme) -> String {
    match value {
        Value::Null => theme.muted("-"),
        Value::Bool(true) => theme.bool_icon(true),
        Value::Bool(false) => theme.bool_icon(false),
        Value::Number(n) => theme.value(&n.to_string()),
        Value::String(s) => theme.value(s),
        Value::Array(items) if items.is_empty() => theme.muted("[]"),
        Value::Array(items) => preview_scalar_list(items, 3, theme.context()),
        Value::Object(map) => format!("{{{} keys}}", map.len()),
    }
}

fn render_table_cell(header: &str, value: &Value, theme: CliTheme) -> String {
    match header.to_ascii_lowercase().as_str() {
        "status" => render_status_cell(value, theme.context()),
        "severity" => render_severity_cell(value, theme.context()),
        "reachable" | "auth_ok" => theme.bool_icon(value.as_bool().unwrap_or(false)),
        "latency_ms" => render_latency(Some(value), theme.context()),
        "version" => value
            .as_str()
            .filter(|s| !s.is_empty())
            .map_or_else(|| theme.muted("-"), |s| theme.value(s)),
        "secret" => render_secret_state(Some(value), theme.context()),
        "url" => match value {
            Value::String(s) if !s.is_empty() => theme.value(s),
            _ => theme.muted("-"),
        },
        _ => render_cell_text(value, theme),
    }
}

#[allow(clippy::option_if_let_else)]
fn render_status_cell(value: &Value, ctx: RenderContext) -> String {
    let theme = CliTheme::from_context(ctx);
    if let Some(status) = value.as_str() {
        match status.to_ascii_lowercase().as_str() {
            "running" | "healthy" | "ok" | "reachable" => theme.ok_badge(),
            "partial" | "warn" | "warning" | "degraded" | "auth" => theme.warn_badge(),
            "stopped" | "down" | "fail" | "failed" | "error" | "unreachable" => theme.error_badge(),
            _ => theme.muted(status),
        }
    } else if value.as_bool().is_some() {
        theme.bool_icon(value.as_bool().unwrap_or(false))
    } else {
        render_cell_text(value, theme)
    }
}

fn render_severity_cell(value: &Value, ctx: RenderContext) -> String {
    let theme = CliTheme::from_context(ctx);
    match value
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "ok" => theme.ok_badge(),
        "warn" | "warning" => theme.warn_badge(),
        "fail" | "error" => theme.error_badge(),
        other => theme.muted(other),
    }
}

fn render_latency(value: Option<&Value>, ctx: RenderContext) -> String {
    let theme = CliTheme::from_context(ctx);
    let Some(value) = value else {
        return theme.muted("-");
    };
    let Some(ms) = value.as_u64() else {
        return render_cell_text(value, theme);
    };
    let text = format!("{ms}ms");
    if ms < 100 {
        theme.value(&text)
    } else if ms < 500 {
        theme.warn(&text)
    } else {
        theme.primary(&text)
    }
}

fn render_secret_state(value: Option<&Value>, ctx: RenderContext) -> String {
    let theme = CliTheme::from_context(ctx);
    match value {
        Some(Value::Bool(false)) => theme.error_badge(),
        Some(Value::Null) | None => theme.muted("-"),
        Some(_) => theme.ok_badge(),
    }
}

fn preview_scalar_list(items: &[Value], limit: usize, ctx: RenderContext) -> String {
    let theme = CliTheme::from_context(ctx);
    let mut parts = Vec::new();
    for item in items.iter().take(limit) {
        parts.push(render_cell_text(item, theme));
    }
    if items.len() > limit {
        parts.push(theme.muted(format!("+{}", items.len() - limit).as_str()));
    }
    parts.join(", ")
}

const fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn is_health_row(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    map.contains_key("service")
        && map.contains_key("reachable")
        && map.contains_key("auth_ok")
        && map.contains_key("latency_ms")
}

fn is_doctor_finding(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    map.contains_key("service")
        && map.contains_key("check")
        && map.contains_key("severity")
        && map.contains_key("message")
}

fn is_doctor_report(map: &serde_json::Map<String, Value>) -> bool {
    map.contains_key("findings")
        && map
            .get("findings")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(is_doctor_finding))
}

fn collect_headers(items: &[Value]) -> Vec<String> {
    let mut headers = Vec::new();
    for item in items {
        if let Some(map) = item.as_object() {
            for key in map.keys() {
                if !headers.contains(key) {
                    headers.push(key.clone());
                }
            }
        }
    }
    headers
}

fn render_doctor_report(
    map: &serde_json::Map<String, Value>,
    theme: CliTheme,
    _indent: usize,
) -> String {
    let findings = map
        .get("findings")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    // Group findings by service, preserving first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, (usize, usize, usize)> =
        std::collections::HashMap::new();
    for item in findings.iter().filter_map(Value::as_object) {
        let Some(service) = item.get("service").and_then(Value::as_str) else {
            continue;
        };
        let severity = item
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let entry = groups.entry(service.to_string()).or_insert_with(|| {
            order.push(service.to_string());
            (0, 0, 0)
        });
        match severity.as_str() {
            "ok" => entry.0 += 1,
            "warn" | "warning" => entry.1 += 1,
            "fail" | "error" => entry.2 += 1,
            _ => {}
        }
    }

    let total_services = order.len();
    let mut healthy = 0usize;
    let mut degraded = 0usize;
    let mut unhealthy = 0usize;
    for name in &order {
        let (_ok, warn, fail) = groups[name];
        if fail > 0 {
            unhealthy += 1;
        } else if warn > 0 {
            degraded += 1;
        } else {
            healthy += 1;
        }
    }

    let mut out = String::new();
    writeln!(out, "{}", theme.heading("Doctor Report")).ok();
    writeln!(
        out,
        "{} {} {} {} {} {} {} {}",
        theme.muted(format!("{total_services} services").as_str()),
        theme.muted("·"),
        theme.ok_badge(),
        theme.muted(format!("{healthy} healthy").as_str()),
        theme.muted("·"),
        theme.warn_badge(),
        theme.muted(format!("{degraded} degraded").as_str()),
        if unhealthy > 0 {
            format!(
                "{} {} {}",
                theme.muted("·"),
                theme.error_badge(),
                theme.muted(format!("{unhealthy} unhealthy").as_str())
            )
        } else {
            String::new()
        },
    )
    .ok();

    if order.is_empty() {
        return out;
    }

    out.push('\n');

    // Build one-row-per-service summary.
    let rows: Vec<Vec<String>> = order
        .iter()
        .map(|service| {
            let (ok, warn, fail) = groups[service];
            let status = if fail > 0 {
                theme.error_badge()
            } else if warn > 0 {
                theme.warn_badge()
            } else {
                theme.ok_badge()
            };
            let total = ok + warn + fail;
            let env_summary = format!(
                "{} {}",
                theme.muted("env"),
                if fail == 0 && warn == 0 {
                    theme.tertiary(format!("{ok}/{total}").as_str())
                } else if fail > 0 {
                    theme.error(format!("{ok}/{total}").as_str())
                } else {
                    theme.warn(format!("{ok}/{total}").as_str())
                }
            );
            vec![format!("  {status}"), theme.display(service), env_summary]
        })
        .collect();

    let headers = vec![String::new(), String::new(), String::new()];
    out.push_str(&render_table_plain(&headers, &rows));
    out
}

/// Render a table without headers or separator — used for borderless list layouts.
fn render_table_plain(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| visible_width(h)).collect();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            if idx < widths.len() {
                widths[idx] = widths[idx].max(visible_width(cell));
            } else {
                widths.push(visible_width(cell));
            }
        }
    }
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&render_table_row(row, &widths, Clone::clone));
    }
    out
}

fn is_catalog(map: &serde_json::Map<String, Value>) -> bool {
    map.len() == 1
        && map
            .get("services")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().filter_map(Value::as_object).any(|svc| {
                    svc.contains_key("name")
                        && svc.contains_key("actions")
                        && svc.contains_key("category")
                })
            })
}

fn render_catalog(map: &serde_json::Map<String, Value>, theme: CliTheme) -> String {
    let services = map
        .get("services")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    let mut out = String::new();
    writeln!(
        out,
        "{} {} {}",
        theme.display("Lab"),
        theme.muted("·"),
        theme.muted(format!("{} services", services.len()).as_str())
    )
    .ok();
    out.push('\n');

    // Measure service-name column width (cap at 14).
    let name_width = services
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|svc| svc.get("name").and_then(Value::as_str))
        .map(str::len)
        .max()
        .unwrap_or(10)
        .min(14);
    let cat_width = services
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|svc| svc.get("category").and_then(Value::as_str))
        .map(str::len)
        .max()
        .unwrap_or(10)
        .min(14);

    // Detail mode: single service requested — list every action vertically.
    // Summary mode: multiple services — compact one-line preview per service.
    let detail_mode = services.len() == 1;
    const ACTION_PREVIEW: usize = 5;
    const MAX_ACTIONS_WIDTH: usize = 64;

    for (idx, svc) in services.iter().filter_map(Value::as_object).enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let name = svc.get("name").and_then(Value::as_str).unwrap_or("-");
        let category = svc.get("category").and_then(Value::as_str).unwrap_or("-");
        let status_str = svc.get("status").and_then(Value::as_str).unwrap_or("");
        let actions = svc
            .get("actions")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let status_icon = match status_str {
            "available" => theme.ok_badge(),
            "stub" => theme.warn_badge(),
            _ => theme.error_badge(),
        };

        writeln!(
            out,
            "  {} {}  {}  {} {}",
            status_icon,
            pad_right(&theme.bold(theme.service_name(name)), name_width),
            pad_right(&theme.secondary(category), cat_width),
            theme.display(actions.len().to_string().as_str()),
            theme.muted("actions"),
        )
        .ok();

        let names: Vec<&str> = actions
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|a| a.get("name").and_then(Value::as_str))
            .collect();
        if names.is_empty() {
            continue;
        }

        if detail_mode {
            // One action per line so all actions are visible without truncation.
            for n in &names {
                writeln!(out, "      {}", theme.tertiary(n)).ok();
            }
        } else {
            // Compact preview: first N names, middle-dot separated, truncate at MAX_ACTIONS_WIDTH.
            let sep = format!(" {} ", theme.muted("·"));
            let indent = "      ";
            let mut line = String::new();
            let mut shown = 0usize;
            for (i, n) in names.iter().take(ACTION_PREVIEW).enumerate() {
                let colored = theme.tertiary(n);
                let candidate = if i == 0 {
                    colored
                } else {
                    format!("{sep}{colored}")
                };
                if visible_width(&line) + visible_width(&candidate) > MAX_ACTIONS_WIDTH {
                    break;
                }
                line.push_str(&candidate);
                shown += 1;
            }
            writeln!(out, "{indent}{line}").ok();
            let remaining = names.len().saturating_sub(shown);
            if remaining > 0 {
                writeln!(
                    out,
                    "{indent}{}",
                    theme.muted(format!("(+{remaining} more — `lab help {name}`)").as_str())
                )
                .ok();
            }
        }
    }

    out
}
fn visible_width(text: &str) -> usize {
    strip_ansi_escapes::strip_str(text).chars().count()
}

fn pad_right(text: &str, width: usize) -> String {
    let visible = visible_width(text);
    let mut padded = String::from(text);
    if visible < width {
        padded.push_str(&" ".repeat(width - visible));
    }
    padded
}

fn indent_str(level: usize) -> String {
    "  ".repeat(level)
}

// Palette — ANSI 256.
// Aurora CLI palette.
//
// Role             Truecolor     ANSI 256
// text primary     #e6f4fb       255
// text muted       #a7bcc9       250
// accent primary   #29b6f6       39
// accent strong    #67cbfa       81
// accent deep      #1c7fac       31
// border default   #1d3d4e       239
// success          #7dd3c7       115
// warning          #c6a36b       180
// error            #c78490       174

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{ColorPolicy, RenderEnv};
    use serde_json::json;

    fn json_format() -> OutputFormat {
        OutputFormat::from_json_flag(true, ColorPolicy::Auto, RenderEnv::stdout())
    }

    fn human_format() -> OutputFormat {
        OutputFormat::from_json_flag(false, ColorPolicy::Plain, RenderEnv::stdout())
    }

    #[test]
    fn json_format_is_compact() {
        let val = json!({"name": "test", "count": 42});
        let out = render(&val, json_format()).unwrap();
        assert!(!out.contains('\n'), "Json format must be single-line");
    }

    #[test]
    fn human_format_is_pretty() {
        let val = json!({"name": "test", "count": 42});
        let out = render(&val, human_format()).unwrap();
        assert!(out.contains('\n'), "Human format must be multi-line");
    }

    #[test]
    fn formats_differ() {
        let val = json!({"a": 1});
        let human = render(&val, human_format()).unwrap();
        let json = render(&val, json_format()).unwrap();
        assert_ne!(human, json, "Human and Json must produce different output");
    }

    #[test]
    fn from_json_flag() {
        assert!(json_format().is_json());
        assert!(human_format().is_human());
    }

    #[test]
    fn health_rows_render_as_status_table() {
        let val = json!([
            {
                "service": "radarr",
                "reachable": true,
                "auth_ok": true,
                "version": "5.17.2",
                "latency_ms": 41,
                "message": "healthy"
            },
            {
                "service": "unifi",
                "reachable": false,
                "auth_ok": false,
                "version": null,
                "latency_ms": 0,
                "message": "not configured"
            }
        ]);

        let out = render(&val, human_format()).unwrap();
        assert!(out.contains("Service Health"));
        assert!(out.contains("radarr"));
        assert!(out.contains("unifi"));
        assert!(out.contains("✓") || out.contains("ok"));
        assert!(out.contains("✗") || out.contains("x"));
    }

    #[test]
    fn doctor_report_renders_summary() {
        let val = json!({
            "findings": [
                {"service": "radarr", "check": "env:RADARR_URL", "severity": "ok", "message": "RADARR_URL is set"},
                {"service": "radarr", "check": "env:RADARR_API_KEY", "severity": "fail", "message": "RADARR_API_KEY is missing"}
            ]
        });

        let out = render(&val, human_format()).unwrap();
        assert!(out.contains("Doctor Report"));
        // Service-grouped summary: per-service row, not per-check.
        assert!(out.contains("radarr"));
        assert!(out.contains("env"));
        assert!(out.contains("✓") || out.contains("ok"));
        assert!(out.contains("✗") || out.contains("x"));
        // Per-check detail is hidden in default (grouped) mode.
        assert!(!out.contains("is set"));
        assert!(!out.contains("is missing"));
    }

    fn make_test_service(name: &str) -> Value {
        serde_json::json!({
            "name": name,
            "description": "Test service",
            "category": "servarr",
            "status": "available",
            "requires_http_subject": false,
            "actions": [
                {"name": "movie.search", "description": "", "destructive": false, "params": [], "returns": ""},
                {"name": "movie.add", "description": "", "destructive": false, "params": [], "returns": ""},
                {"name": "queue.list", "description": "", "destructive": false, "params": [], "returns": ""},
                {"name": "queue.purge", "description": "", "destructive": true, "params": [], "returns": ""},
                {"name": "history.list", "description": "", "destructive": false, "params": [], "returns": ""},
                {"name": "root.list", "description": "", "destructive": false, "params": [], "returns": ""},
                {"name": "tag.list", "description": "", "destructive": false, "params": [], "returns": ""}
            ]
        })
    }

    #[test]
    fn catalog_summary_shows_truncation_hint() {
        // Two services → summary mode → compact preview with "(+N more — `lab help X`)" hint.
        let val = serde_json::json!({
            "services": [make_test_service("alpha"), make_test_service("beta")]
        });
        let out = render(&val, human_format()).unwrap();
        let plain = strip_ansi_escapes::strip_str(&out);
        assert!(plain.contains("Lab"));
        assert!(plain.contains("alpha"));
        assert!(plain.contains("servarr"));
        assert!(plain.contains("7 actions"));
        assert!(plain.contains("movie.search"));
        assert!(plain.contains("(+"));
        assert!(plain.contains("more"));
        assert!(plain.contains("lab help alpha"));
        assert!(
            !plain.contains("{5 keys}"),
            "nested ActionEntry rendered as '{{N keys}}' artifact"
        );
        assert!(!plain.contains("keys}"), "any 'keys}}' artifact leaked");
    }

    #[test]
    fn catalog_detail_shows_all_actions_without_hint() {
        // Single service → detail mode → all actions listed, no truncation hint.
        let val = serde_json::json!({
            "services": [make_test_service("alpha")]
        });
        let out = render(&val, human_format()).unwrap();
        let plain = strip_ansi_escapes::strip_str(&out);
        assert!(plain.contains("Lab"));
        assert!(plain.contains("alpha"));
        assert!(plain.contains("7 actions"));
        assert!(plain.contains("movie.search"));
        assert!(
            plain.contains("tag.list"),
            "all actions should be visible in detail mode"
        );
        assert!(
            !plain.contains("(+"),
            "detail mode must not show truncation hint"
        );
        assert!(
            !plain.contains("lab help alpha"),
            "detail mode must not show hint to itself"
        );
        assert!(
            !plain.contains("{5 keys}"),
            "nested ActionEntry rendered as '{{N keys}}' artifact"
        );
        assert!(!plain.contains("keys}"), "any 'keys}}' artifact leaked");
    }
}
