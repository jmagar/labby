#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
#[test]
fn scans_claude_codex_and_gemini_configs_when_present() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".codex")).unwrap();
    std::fs::write(
        temp.path().join(".claude.json"),
        r#"{"mcpServers":{"labby":{"command": "labby","args":["serve"]}}}"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join(".codex/config.toml"),
        r#"[mcp_servers.lab]
command = "lab"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join(".gemini")).unwrap();
    std::fs::write(
        temp.path().join(".gemini/settings.json"),
        r#"{"mcpServers":{"labby":{"url":"http://127.0.0.1:8765/mcp"}}}"#,
    )
    .unwrap();

    let inventory = labby::node::config_scan::discover_ai_cli_configs(temp.path()).unwrap();
    assert_eq!(inventory.len(), 3);
    assert!(inventory.iter().all(|entry| !entry.content_hash.is_empty()));
    assert_eq!(
        inventory
            .iter()
            .map(|entry| entry.path.display().to_string())
            .collect::<Vec<_>>(),
        vec![".claude.json", "config.toml", "settings.json"]
    );
    assert!(inventory.iter().all(|entry| {
        entry.servers.values().all(|server| {
            !server.fingerprint.is_empty()
                && !matches!(server.transport.as_deref(), Some("lab" | "serve"))
        })
    }));
}

#[test]
fn skips_non_file_ai_cli_config_paths() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".claude.json")).unwrap();
    std::fs::create_dir_all(temp.path().join(".codex/config.toml")).unwrap();

    let inventory = labby::node::config_scan::discover_ai_cli_configs(temp.path()).unwrap();
    assert!(inventory.is_empty());
}
