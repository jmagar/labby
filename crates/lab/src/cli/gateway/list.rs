use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;

use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::gateway::view_models::ServerView;
use crate::output::OutputFormat;

/// Render `gateway list` with a typed per-server layout instead of the generic
/// Value-shape table (which renders nested objects as `{N keys}` placeholders).
///
/// Format inspired by `claude mcp list` (status icon + one-line per server)
/// and `codex mcp list` (column alignment). JSON mode preserves the full
/// `ServerView` shape for downstream consumers.
pub(super) async fn run_gateway_list(
    manager: Arc<GatewayManager>,
    format: OutputFormat,
) -> Result<ExitCode> {
    let servers = match manager.list().await {
        Ok(s) => s,
        Err(err) => {
            return Err(anyhow::anyhow!(
                "{}",
                serde_json::to_string(&err).unwrap_or_else(|_| err.to_string())
            ));
        }
    };

    if format.is_json() {
        #[allow(clippy::print_stdout)]
        {
            println!(
                "{}",
                serde_json::to_string_pretty(&servers).unwrap_or_else(|_| "[]".to_string())
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    render_gateway_list_human(&servers, format);
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::print_stdout)]
fn render_gateway_list_human(servers: &[ServerView], format: OutputFormat) {
    use crate::output::theme::CliTheme;
    let theme = CliTheme::from_context(format.render_context());

    let mut connected = 0usize;
    let mut failed = 0usize;
    let mut disabled = 0usize;
    for s in servers {
        if !s.enabled {
            disabled += 1;
        } else if s.connected {
            connected += 1;
        } else {
            failed += 1;
        }
    }

    let total = servers.len();
    println!(
        "{} {}",
        theme.section(&format!("Lab Gateway · {total} servers")),
        theme.muted(format!(
            "({} connected, {} disconnected, {} disabled)",
            connected, failed, disabled
        )),
    );
    println!();

    if servers.is_empty() {
        println!("  {}", theme.muted("no servers configured"));
        return;
    }

    let mut servers: Vec<&ServerView> = servers.iter().collect();
    servers.sort_by_key(|s| {
        if !s.enabled {
            2u8
        } else {
            u8::from(!s.connected)
        }
    });
    let servers = servers.as_slice();

    let name_width = servers
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(8);
    let transport_width = servers
        .iter()
        .filter_map(|s| s.config_summary.transport.as_deref())
        .map(|t| t.chars().count())
        .max()
        .unwrap_or(5)
        .max(5);

    for s in servers {
        let icon = if !s.enabled {
            theme.muted("⊘")
        } else if s.connected {
            theme.ok_badge()
        } else {
            theme.warn_badge()
        };

        let name_padded = format!("{:width$}", s.name, width = name_width);
        let name = if s.connected {
            theme.primary(&name_padded)
        } else if !s.enabled {
            theme.muted(&name_padded)
        } else {
            theme.warn(&name_padded)
        };

        let transport_raw = s.config_summary.transport.as_deref().unwrap_or("—");
        let transport_padded = format!("{:width$}", transport_raw, width = transport_width);
        let transport = theme.tertiary(&transport_padded);

        let status_detail = if !s.enabled {
            theme.muted("disabled")
        } else if s.connected {
            let mut parts = Vec::new();
            if s.exposed_tool_count > 0 {
                parts.push(format!("🔧 {}", s.exposed_tool_count));
            }
            if s.exposed_prompt_count > 0 {
                parts.push(format!("💬 {}", s.exposed_prompt_count));
            }
            if s.exposed_resource_count > 0 {
                parts.push(format!("📦 {}", s.exposed_resource_count));
            }
            let joined = if parts.is_empty() {
                "connected".to_string()
            } else {
                parts.join(" · ")
            };
            theme.secondary(&joined)
        } else {
            let msg = s
                .warnings
                .iter()
                .find(|w| !w.message.is_empty())
                .map(|w| {
                    let m = w.message.trim();
                    if m.len() > 80 {
                        format!("{}…", &m[..80])
                    } else {
                        m.to_string()
                    }
                })
                .unwrap_or_else(|| "not connected".to_string());
            theme.warn(&msg)
        };

        let location_raw = match s.config_summary.command.as_deref() {
            Some(command) if !command.is_empty() => {
                let mut line = command.to_string();
                if !s.config_summary.args.is_empty() {
                    line.push(' ');
                    line.push_str(&s.config_summary.args.join(" "));
                }
                Some(line)
            }
            _ => s
                .config_summary
                .target
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        };
        let tail = match (location_raw, s.pid) {
            (Some(loc), Some(pid)) => theme.muted(format!("{loc} · pid {pid}")),
            (Some(loc), None) => theme.muted(loc),
            (None, Some(pid)) => theme.muted(format!("pid {pid}")),
            (None, None) => String::new(),
        };

        println!("  {icon} {name}  {transport}  {status_detail}  {tail}");
    }
}
