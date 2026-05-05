use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;

use crate::registry::{build_default_registry, service_meta};

const DEFAULT_ORG: &str = match option_env!("LAB_PLUGIN_ORG") {
    Some(value) => value,
    None => "lab",
};
const CORE_PLUGIN: &str = "lab-core";
const CORE_BINARY_NAME: &str = "labby";
const CORE_BINARY_PATH: &str = "${HOME}/.claude/plugins/lab-core/bin/labby";

#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Output directory for the generated marketplace tree.
    #[arg(long)]
    pub out: PathBuf,
    /// Marketplace/org suffix used in plugin ids, for example `lab`.
    #[arg(long, default_value = DEFAULT_ORG)]
    pub org: String,
    /// Release binary to copy into lab-core/bin/labby.
    #[arg(long)]
    pub binary: Option<PathBuf>,
}

pub fn run_generate(args: GenerateArgs) -> Result<ExitCode> {
    let binary = args.binary.unwrap_or_else(default_binary_path);
    generate_marketplace(&args.out, &args.org, &binary)?;
    println!("generated marketplace at {}", args.out.display());
    Ok(ExitCode::SUCCESS)
}

fn generate_marketplace(out: &Path, org: &str, binary: &Path) -> Result<()> {
    if !binary.is_file() {
        anyhow::bail!(
            "release binary not found at {}; run `just build-release` or pass --binary",
            binary.display()
        );
    }
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;

    let registry = build_default_registry();
    let mut service_names = registry
        .services()
        .iter()
        .filter_map(|entry| service_meta(entry.name).map(|_| entry.name.to_string()))
        .collect::<Vec<_>>();
    service_names.sort();

    write_core_plugin(out, org, binary)?;
    for service in &service_names {
        write_service_plugin(out, org, service)?;
    }
    write_marketplace_manifest(out, org, &service_names)?;
    Ok(())
}

fn write_core_plugin(out: &Path, org: &str, binary: &Path) -> Result<()> {
    let root = out.join(CORE_PLUGIN);
    fs::create_dir_all(root.join(".claude-plugin"))?;
    fs::create_dir_all(root.join("bin"))?;
    fs::create_dir_all(root.join("commands"))?;
    fs::create_dir_all(root.join("skills/install-binary"))?;

    let manifest = plugin_manifest(
        CORE_PLUGIN,
        "Core Labby binary and setup commands for Claude Code service plugins.",
        org,
        &["labby", "setup", "homelab", "mcp"],
    );
    write_json(&root.join(".claude-plugin/plugin.json"), &manifest)?;
    write_json(&root.join("plugin.json"), &manifest)?;
    fs::write(root.join("README.md"), core_readme(org))?;
    fs::write(
        root.join("commands/setup-core.md"),
        setup_core_command(false),
    )?;
    fs::write(
        root.join("commands/setup-core-advanced.md"),
        setup_core_command(true),
    )?;
    fs::write(
        root.join("skills/install-binary/SKILL.md"),
        install_binary_skill(),
    )?;

    let dest = root.join("bin").join(CORE_BINARY_NAME);
    fs::copy(binary, &dest).with_context(|| {
        format!(
            "copy release binary from {} to {}",
            binary.display(),
            dest.display()
        )
    })?;
    set_executable(&dest)?;
    Ok(())
}

fn write_service_plugin(out: &Path, org: &str, service: &str) -> Result<()> {
    let Some(meta) = service_meta(service) else {
        return Ok(());
    };
    let plugin_name = format!("lab-{service}");
    let root = out.join(&plugin_name);
    fs::create_dir_all(root.join(".claude-plugin"))?;
    fs::create_dir_all(root.join("commands"))?;

    let manifest = plugin_manifest(
        &plugin_name,
        meta.description,
        org,
        &[service, "labby", "mcp", "homelab"],
    );
    write_json(&root.join(".claude-plugin/plugin.json"), &manifest)?;
    write_json(&root.join("plugin.json"), &manifest)?;
    write_json(
        &root.join(".mcp.json"),
        &json!({
            "mcpServers": {
                service: {
                    "command": CORE_BINARY_PATH,
                    "args": ["mcp", "--services", service]
                }
            }
        }),
    )?;
    fs::write(root.join("README.md"), service_readme(service, org))?;
    fs::write(
        root.join("commands/install-core.md"),
        install_core_command(org),
    )?;
    Ok(())
}

fn write_marketplace_manifest(out: &Path, org: &str, services: &[String]) -> Result<()> {
    let mut plugins = Vec::with_capacity(services.len() + 1);
    plugins.push(json!({
        "name": CORE_PLUGIN,
        "source": format!("./{CORE_PLUGIN}"),
        "description": "Core Labby binary and setup commands for Claude Code service plugins."
    }));
    for service in services {
        let Some(meta) = service_meta(service) else {
            continue;
        };
        plugins.push(json!({
            "name": format!("lab-{service}"),
            "source": format!("./lab-{service}"),
            "description": meta.description
        }));
    }
    let manifest = json!({
        "$schema": "https://json.schemastore.org/claude-code-marketplace.json",
        "name": org,
        "owner": {
            "name": "Labby",
            "email": "noreply@example.invalid"
        },
        "description": "Generated Labby Claude Code service plugins.",
        "plugins": plugins
    });
    write_json(&out.join("plugin-marketplace.json"), &manifest)?;
    fs::create_dir_all(out.join(".claude-plugin"))?;
    write_json(&out.join(".claude-plugin/marketplace.json"), &manifest)?;
    Ok(())
}

fn plugin_manifest(
    name: &str,
    description: &str,
    org: &str,
    keywords: &[&str],
) -> serde_json::Value {
    json!({
        "$schema": "https://json.schemastore.org/claude-code-plugin-manifest.json",
        "name": name,
        "description": description,
        "author": {
            "name": "Labby",
            "email": "noreply@example.invalid"
        },
        "repository": "https://github.com/jmagar/lab",
        "homepage": "https://github.com/jmagar/lab",
        "license": "MIT OR Apache-2.0",
        "keywords": keywords,
        "metadata": {
            "marketplace": org
        }
    })
}

fn core_readme(org: &str) -> String {
    format!(
        "# lab-core\n\nCore Labby plugin for Claude Code.\n\nCommands:\n\n- `/setup-core` runs `labby setup --mode plugin` for the plugin-focused setup flow.\n- `/setup-core-advanced` runs `labby setup --mode full` for the full operator setup flow.\n\nThe bundled binary lives at `bin/labby`. Service plugins call it directly from `{CORE_BINARY_PATH}` so they do not depend on PATH.\n\nInstall service plugins as `lab-<service>@{org}` after installing this core plugin.\n"
    )
}

fn service_readme(service: &str, org: &str) -> String {
    let Some(meta) = service_meta(service) else {
        return String::new();
    };
    let required = if meta.required_env.is_empty() {
        "- none\n".to_string()
    } else {
        meta.required_env
            .iter()
            .map(|var| format!("- `{}` - {}\n", var.name, var.description))
            .collect::<String>()
    };
    let optional = if meta.optional_env.is_empty() {
        "- none\n".to_string()
    } else {
        meta.optional_env
            .iter()
            .map(|var| format!("- `{}` - {}\n", var.name, var.description))
            .collect::<String>()
    };
    format!(
        "# lab-{service}\n\n{}\n\nThis plugin starts Labby with only `{service}` enabled:\n\n```json\n{{ \"command\": \"{CORE_BINARY_PATH}\", \"args\": [\"mcp\", \"--services\", \"{service}\"] }}\n```\n\nRun `/setup-core` to fill in service credentials.\n\nIf `lab-core` is not installed, run:\n\n```bash\nclaude plugin install lab-core@{org}\n```\n\n## Required env vars\n\n{required}\n## Optional env vars\n\n{optional}",
        meta.description
    )
}

fn setup_core_command(advanced: bool) -> String {
    let (description, mode) = if advanced {
        ("Open the full Labby operator setup flow.", "full")
    } else {
        ("Open the plugin-focused Labby setup flow.", "plugin")
    };
    format!(
        "---\ndescription: {description}\nallowed-tools: Bash(labby setup:*)\n---\n\nRun the Labby setup flow:\n\n```bash\nlabby setup --mode {mode}\n```\n"
    )
}

fn install_core_command(org: &str) -> String {
    format!(
        "---\ndescription: Print the command that installs the Labby core plugin.\n---\n\nInstall the Labby core plugin, then restart Claude Code:\n\n```bash\nclaude plugin install lab-core@{org}\n```\n"
    )
}

fn install_binary_skill() -> &'static str {
    r#"---
name: install-binary
description: Ensure the bundled labby binary is reachable from ~/.local/bin.
---

# Install Binary

If `~/.local/bin/labby` does not exist or does not point at `${CLAUDE_PLUGIN_ROOT}/bin/labby`, offer to create a symlink.

Use:

```bash
mkdir -p ~/.local/bin
ln -sfn "${CLAUDE_PLUGIN_ROOT}/bin/labby" ~/.local/bin/labby
```

If symlink creation fails, tell the user that the core binary is still available at `${CLAUDE_PLUGIN_ROOT}/bin/labby`; service plugins use that absolute plugin path directly and do not require PATH.

Never install other plugins, edit Claude Code config, or restart services.
"#
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

fn default_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/release/labby")
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_readme_lists_required_env_vars() {
        let readme = service_readme("radarr", "lab");
        assert!(readme.contains("RADARR_URL"));
        assert!(readme.contains("RADARR_API_KEY"));
        assert!(readme.contains("/setup-core"));
    }

    #[test]
    fn service_mcp_path_has_no_path_dependency() {
        assert_eq!(
            CORE_BINARY_PATH,
            "${HOME}/.claude/plugins/lab-core/bin/labby"
        );
    }
}
