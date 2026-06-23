//! Upstream process cleanup pattern + matcher tests.
#![allow(clippy::panic)]

#[cfg(target_os = "linux")]
use crate::gateway::runtime::process_matches_patterns;
use crate::gateway::runtime::upstream_cleanup_patterns;

use super::*;

#[test]
fn github_chat_cleanup_patterns_cover_uv_wrappers() {
    let upstream = UpstreamConfig {
        enabled: true,
        name: "github-chat".to_string(),
        url: None,
        bearer_token_env: None,
        command: Some("uvx".to_string()),
        args: vec!["github-chat-mcp".to_string()],
        env: BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };

    let patterns = upstream_cleanup_patterns(&upstream, false);
    assert!(patterns.contains(&"github-chat-mcp".to_string()));
    assert!(patterns.contains(&"uvx github-chat-mcp".to_string()));
    assert!(patterns.contains(&"uv tool uvx github-chat-mcp".to_string()));
    assert!(patterns.contains(&"uv run github-chat-mcp".to_string()));
    assert!(patterns.contains(&"github-chat".to_string()));
}

#[cfg(target_os = "linux")]
#[test]
fn process_matcher_uses_joined_cmdline_text() {
    let patterns = vec!["uvx github-chat-mcp".to_string(), "github-chat".to_string()];
    assert!(process_matches_patterns(
        "uvx github-chat-mcp --transport stdio",
        &patterns,
    ));
    assert!(!process_matches_patterns(
        "python -m unrelated-service",
        &patterns,
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cleanup_upstream_processes_kills_matching_github_chat_runtime() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    let upstream_name = "github-chat-cleanup-manager";
    let runtime_arg = "github-chat-cleanup-manager-mcp";

    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The cleanup path kills process groups for child runtimes. Keep this
    // stand-in out of nextest's process group so the test process survives.
    command.process_group(0);
    let mut child = command.spawn().expect("spawn github chat stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;

    let _cleanup = manager
        .cleanup_upstream_processes(upstream_name, false, false)
        .await
        .expect("cleanup");

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("github-chat stand-in process was not terminated by cleanup");
}
