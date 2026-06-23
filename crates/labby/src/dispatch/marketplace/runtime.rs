#![allow(dead_code)]

use labby_apis::marketplace::MarketplaceRuntime;

use crate::dispatch::error::ToolError;

#[must_use]
pub const fn runtime_display_name(runtime: MarketplaceRuntime) -> &'static str {
    match runtime {
        MarketplaceRuntime::Claude => "Claude Code",
        MarketplaceRuntime::Codex => "Codex",
        MarketplaceRuntime::Gemini => "Gemini CLI",
    }
}

pub fn parse_marketplace_runtime(value: &str) -> Result<MarketplaceRuntime, ToolError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => Ok(MarketplaceRuntime::Claude),
        "codex" => Ok(MarketplaceRuntime::Codex),
        "gemini" | "gemini-cli" => Ok(MarketplaceRuntime::Gemini),
        _ => Err(ToolError::InvalidParam {
            message: format!("unsupported marketplace runtime `{value}`"),
            param: "runtime".into(),
        }),
    }
}
