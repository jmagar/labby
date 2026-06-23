//! Pure matching logic for discovery imports: tombstone matching, the
//! discovered-server partition used by auto-import, and the tombstone selector.

use std::collections::HashSet;

use crate::gateway::discovery::DiscoveredServer;
use crate::gateway::types::{ImportResultView, ImportSkipReason, ImportSkipView};
use lab_runtime::gateway_config::{GatewayConfig, UpstreamConfig, UpstreamImportTombstone};

fn tombstone_matches_discovered(
    tombstone: &UpstreamImportTombstone,
    server: &DiscoveredServer,
) -> bool {
    tombstone_discovery_source_matches(tombstone, server)
        && (tombstone.name == server.name
            || tombstone
                .imported_from
                .server_name
                .as_deref()
                .is_some_and(|name| name == server.name))
        && tombstone_transport_matches_discovered(tombstone, server)
}

fn tombstone_discovery_source_matches(
    tombstone: &UpstreamImportTombstone,
    server: &DiscoveredServer,
) -> bool {
    tombstone.imported_from.client == server.source_client
        && tombstone.imported_from.path == server.source_path
}

fn tombstone_transport_matches_discovered(
    tombstone: &UpstreamImportTombstone,
    server: &DiscoveredServer,
) -> bool {
    let Some(tombstone_fingerprint) = tombstone.imported_from.transport_fingerprint.as_deref()
    else {
        return true;
    };
    server
        .spec
        .imported_from
        .as_ref()
        .and_then(|source| source.transport_fingerprint.as_deref())
        .is_some_and(|fingerprint| fingerprint == tombstone_fingerprint)
}

pub(crate) fn partition_discovered_for_import(
    cfg: &GatewayConfig,
    discovered: Vec<DiscoveredServer>,
) -> (ImportResultView, Vec<UpstreamConfig>) {
    let already: HashSet<&str> = cfg.upstream.iter().map(|u| u.name.as_str()).collect();

    let mut result = ImportResultView::default();
    let mut specs_to_add = Vec::new();
    for server in discovered {
        if already.contains(server.name.as_str()) {
            result.skipped.push(ImportSkipView {
                name: server.name,
                reason: ImportSkipReason::AlreadyConfigured,
            });
        } else if cfg
            .upstream_import_tombstones
            .iter()
            .any(|tombstone| tombstone_matches_discovered(tombstone, &server))
        {
            result.skipped.push(ImportSkipView {
                name: server.name,
                reason: ImportSkipReason::Tombstoned,
            });
        } else {
            specs_to_add.push(server.spec);
        }
    }

    (result, specs_to_add)
}

pub(crate) fn discovered_is_tombstoned(cfg: &GatewayConfig, server: &DiscoveredServer) -> bool {
    cfg.upstream_import_tombstones
        .iter()
        .any(|tombstone| tombstone_matches_discovered(tombstone, server))
}

#[derive(Debug, Clone, Default)]
pub struct ImportTombstoneSelector {
    pub name: String,
    pub source_client: Option<String>,
    pub source_path: Option<String>,
    pub server_name: Option<String>,
    pub transport_fingerprint: Option<String>,
}

impl ImportTombstoneSelector {
    pub(super) fn matches_tombstone(&self, tombstone: &UpstreamImportTombstone) -> bool {
        if tombstone.name != self.name
            && tombstone.imported_from.server_name.as_deref() != Some(self.name.as_str())
        {
            return false;
        }
        if let Some(source_client) = self.source_client.as_deref()
            && tombstone.imported_from.client != source_client
        {
            return false;
        }
        if let Some(source_path) = self.source_path.as_deref()
            && tombstone.imported_from.path != source_path
        {
            return false;
        }
        if let Some(server_name) = self.server_name.as_deref()
            && tombstone.imported_from.server_name.as_deref() != Some(server_name)
        {
            return false;
        }
        if let Some(fingerprint) = self.transport_fingerprint.as_deref()
            && tombstone.imported_from.transport_fingerprint.as_deref() != Some(fingerprint)
        {
            return false;
        }
        true
    }

    pub(super) fn matches_discovered(&self, server: &DiscoveredServer) -> bool {
        if server.name != self.name {
            return false;
        }
        if let Some(source_client) = self.source_client.as_deref()
            && server.source_client != source_client
        {
            return false;
        }
        if let Some(source_path) = self.source_path.as_deref()
            && server.source_path != source_path
        {
            return false;
        }
        if let Some(server_name) = self.server_name.as_deref()
            && server
                .spec
                .imported_from
                .as_ref()
                .and_then(|source| source.server_name.as_deref())
                != Some(server_name)
        {
            return false;
        }
        if let Some(fingerprint) = self.transport_fingerprint.as_deref()
            && server
                .spec
                .imported_from
                .as_ref()
                .and_then(|source| source.transport_fingerprint.as_deref())
                != Some(fingerprint)
        {
            return false;
        }
        true
    }
}
