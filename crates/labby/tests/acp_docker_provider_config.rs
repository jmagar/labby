#![cfg(feature = "acp")]
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
use serde_json::Value;

#[test]
fn docker_codex_acp_disables_inner_sandbox() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let path = root.join("config/acp-providers.docker.json");
    let text = std::fs::read_to_string(&path).expect("read docker ACP provider config");
    let providers: Vec<Value> =
        serde_json::from_str(&text).expect("parse docker ACP provider config");

    let codex = providers
        .iter()
        .find(|provider| provider["id"] == "codex-acp")
        .expect("codex-acp provider exists");
    let args = codex["args"].as_array().expect("codex-acp args");

    assert!(
        args.iter()
            .any(|arg| arg.as_str() == Some("sandbox_mode=\"danger-full-access\"")),
        "Docker Codex ACP must run with sandbox_mode=\"danger-full-access\" because Docker \
         seccomp blocks the nested namespace sandbox used by workspace-write/read-only modes"
    );
}
