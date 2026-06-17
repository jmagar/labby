use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::cli::gateway::GatewayOauthUpstreamArgs;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::output::OutputFormat;

#[derive(Debug, Deserialize)]
struct GatewayOauthStartView {
    authorization_url: String,
}

pub(super) async fn run_gateway_oauth_start(
    manager: Arc<GatewayManager>,
    args: GatewayOauthUpstreamArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    let params = json!({ "upstream": args.name, "subject": args.subject });
    let start = std::time::Instant::now();
    let value =
        crate::dispatch::gateway::dispatch_with_manager(&manager, "gateway.oauth.start", params)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "{}",
                    serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
                )
            })?;
    tracing::info!(
        surface = "cli",
        service = "gateway",
        action = "gateway.oauth.start",
        elapsed_ms = start.elapsed().as_millis(),
        "dispatch ok"
    );

    if format.is_json() {
        crate::output::print(&value, format)?;
    }

    let start_view: GatewayOauthStartView =
        serde_json::from_value(value.clone()).map_err(|error| {
            anyhow::anyhow!("failed to decode gateway oauth start response: {error}")
        })?;

    let theme = crate::output::theme::CliTheme::from_context(format.render_context());

    if args.open {
        open_in_browser(&start_view.authorization_url)?;
        eprintln!(
            "{}",
            theme.muted("Opened authorization URL in your browser.")
        );
    } else {
        eprintln!(
            "{}\n{}",
            theme.muted("Open this URL to authorize:"),
            theme.accent(&start_view.authorization_url)
        );
    }

    if args.wait {
        let subject = args
            .subject
            .as_deref()
            .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
        eprintln!(
            "{}",
            theme.muted(format!(
                "Waiting for OAuth completion for `{}` using shared subject `{}`...",
                args.name, subject
            ))
        );
        let wait_value = crate::dispatch::gateway::dispatch_with_manager(
            &manager,
            "gateway.oauth.wait",
            json!({
                "upstream": args.name,
                "subject": subject,
                "timeout_secs": args.wait_timeout_secs,
            }),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{}",
                serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
            )
        })?;

        let authenticated = wait_value
            .get("authenticated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if authenticated {
            eprintln!(
                "{}",
                theme.success(&format!(
                    "OAuth completed for `{}`. The existing callback route stored credentials for shared subject `{}`.",
                    args.name, subject
                ))
            );
        } else {
            eprintln!(
                "{}",
                theme.warn(&format!(
                    "Timed out waiting for OAuth completion for `{}` after {}s. The browser callback may still succeed later; re-run `labby gateway mcp auth status {}` to check.",
                    args.name, args.wait_timeout_secs, args.name
                ))
            );
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "opening a browser is not supported on this platform"
    ))
}
