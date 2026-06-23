//! Auto-import partition + tombstone matching tests.

use crate::gateway::manager::partition_discovered_for_import;
use crate::gateway::types::ImportSkipReason;
use labby_runtime::gateway_config::UpstreamImportTombstone;

use super::*;

#[test]
fn auto_import_partition_skips_name_tombstoned_discovered_server() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "removed-server",
            fixture_import_source("removed-server"),
        )],
        ..GatewayConfig::default()
    };

    let (result, specs_to_add) =
        partition_discovered_for_import(&cfg, vec![fixture_discovered_http("removed-server")]);

    assert!(specs_to_add.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].name, "removed-server");
    assert_eq!(result.skipped[0].reason, ImportSkipReason::Tombstoned);
}

#[test]
fn auto_import_partition_skips_source_tombstone_after_lab_rename() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "renamed-in-lab",
            fixture_import_source("original-config-name"),
        )],
        ..GatewayConfig::default()
    };

    let (result, specs_to_add) = partition_discovered_for_import(
        &cfg,
        vec![fixture_discovered_http("original-config-name")],
    );

    assert!(specs_to_add.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].name, "original-config-name");
    assert_eq!(result.skipped[0].reason, ImportSkipReason::Tombstoned);
}

#[test]
fn auto_import_partition_does_not_source_match_legacy_tombstone_without_server_name() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "old-removed-server",
            ImportSource::new(
                "codex",
                "/home/alice/.codex/config.toml",
                "2026-05-15T00:00:00Z",
            ),
        )],
        ..GatewayConfig::default()
    };

    let (result, specs_to_add) = partition_discovered_for_import(
        &cfg,
        vec![fixture_discovered_http("different-server-same-file")],
    );

    assert!(result.skipped.is_empty());
    assert_eq!(specs_to_add.len(), 1);
    assert_eq!(specs_to_add[0].name, "different-server-same-file");
}

#[test]
fn auto_import_partition_does_not_tombstone_same_name_from_different_source() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "shared-name",
            fixture_import_source("shared-name"),
        )],
        ..GatewayConfig::default()
    };
    let mut discovered = fixture_discovered_http("shared-name");
    discovered.source_client = "claude-code".to_string();
    discovered.source_path = "/home/alice/.claude/settings.json".to_string();

    let (result, specs_to_add) = partition_discovered_for_import(&cfg, vec![discovered]);

    assert!(result.skipped.is_empty());
    assert_eq!(specs_to_add.len(), 1);
    assert_eq!(specs_to_add[0].name, "shared-name");
}

#[test]
fn auto_import_partition_does_not_tombstone_same_source_when_fingerprint_changes() {
    let source =
        fixture_import_source("fingerprinted").with_transport_fingerprint("old-fingerprint");
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now("fingerprinted", source)],
        ..GatewayConfig::default()
    };
    let mut discovered = fixture_discovered_http("fingerprinted");
    discovered.spec.imported_from =
        Some(fixture_import_source("fingerprinted").with_transport_fingerprint("new-fingerprint"));

    let (result, specs_to_add) = partition_discovered_for_import(&cfg, vec![discovered]);

    assert!(result.skipped.is_empty());
    assert_eq!(specs_to_add.len(), 1);
    assert_eq!(specs_to_add[0].name, "fingerprinted");
}
