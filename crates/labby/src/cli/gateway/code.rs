use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use labby_codemode::MAX_SOURCE_BYTES;

use crate::cli::gateway::{GatewayCodeArgs, GatewayCodeCommand};
use crate::dispatch::gateway::code_mode::{CodeModeBroker, CodeModeCaller, CodeModeSurface};
use crate::dispatch::gateway::manager::GatewayManager;
use crate::output::OutputFormat;

pub(super) async fn run_gateway_code(
    manager: Arc<GatewayManager>,
    args: GatewayCodeArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    let broker = CodeModeBroker::new(Some(manager.as_ref()));
    let caller = CodeModeCaller::TrustedLocal;
    let surface = CodeModeSurface::Cli;

    match args.command {
        GatewayCodeCommand::Status => {
            crate::output::print(&manager.code_mode_config().await, format)?;
        }
        GatewayCodeCommand::Enable => {
            let mut next = manager.code_mode_config().await;
            next.enabled = true;
            let updated = manager.set_code_mode_config(next, None, None).await?;
            crate::output::print(&updated, format)?;
        }
        GatewayCodeCommand::Disable => {
            let mut next = manager.code_mode_config().await;
            next.enabled = false;
            let updated = manager.set_code_mode_config(next, None, None).await?;
            crate::output::print(&updated, format)?;
        }
        GatewayCodeCommand::Exec { code, file } => {
            let code = read_code_mode_source(code, file, MAX_SOURCE_BYTES as u64)?;
            let config = manager.code_mode_config().await;
            let response = broker
                .execute(
                    &code,
                    caller,
                    surface,
                    config,
                    crate::dispatch::gateway::code_mode::ToolScope::default(),
                )
                .await?;
            crate::output::print(&response, format)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn read_code_mode_source(
    code: Option<String>,
    file: Option<std::path::PathBuf>,
    max_source_bytes: u64,
) -> Result<String> {
    match (code, file) {
        (Some(code), None) => {
            // Check the inline string length before any further buffering.
            if code.len() as u64 > max_source_bytes {
                anyhow::bail!("Code Mode source exceeds {max_source_bytes} bytes");
            }
            Ok(code)
        }
        (None, Some(path)) => {
            let metadata = std::fs::metadata(&path)?;
            if metadata.len() > max_source_bytes {
                anyhow::bail!("Code Mode source file exceeds {max_source_bytes} bytes");
            }
            use std::io::Read as _;
            let mut buf = String::new();
            std::fs::File::open(&path)?
                .take(max_source_bytes + 1)
                .read_to_string(&mut buf)?;
            if buf.len() as u64 > max_source_bytes {
                anyhow::bail!("Code Mode source file exceeds {max_source_bytes} bytes");
            }
            Ok(buf)
        }
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_source_limit_is_shared_const_boundary() {
        let at_limit = "a".repeat(MAX_SOURCE_BYTES);
        assert!(read_code_mode_source(Some(at_limit), None, MAX_SOURCE_BYTES as u64).is_ok());

        let over_limit = "a".repeat(MAX_SOURCE_BYTES + 1);
        assert!(read_code_mode_source(Some(over_limit), None, MAX_SOURCE_BYTES as u64).is_err());
    }
}
