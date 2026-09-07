//! Surface-neutral canonical Skills registry orchestration.
//!
//! This module combines first-party/operator skills with route-scoped upstream
//! skills without depending on MCP request types. Native SEP handlers, the
//! compatibility service, CLI, and API all consume this facade.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use labby_runtime::artifacts::{
    LibraryActorId, LibraryOwnerKind, LibraryTenantId, SkillVisibility,
};
use labby_runtime::error::ToolError;
use labby_runtime::skills::parse_skill_uri;
use labby_runtime::skills::wire::{
    CACHE_SCOPE_PRIVATE, CACHE_SCOPE_PUBLIC, SkillEntry, SkillsListResult,
};
#[cfg(feature = "gateway")]
use labby_runtime::skills::{
    SkillDiscoverRequest, SkillGetRequest, SkillId, SkillProvider, SkillProviderDeadline,
    SkillResourceReadRequest,
};
use labby_runtime::skills::{SkillProviderEntry, SkillProviderError, limits};

#[cfg(feature = "gateway")]
use futures::{StreamExt, stream};
#[cfg(feature = "gateway")]
use labby_gateway::gateway::manager::GatewayManager;
#[cfg(feature = "gateway")]
use labby_gateway::upstream::pool::{SepSkillProvider, UpstreamPool};

use super::aggregate::{self, ToolAccess};
use super::registry::{FirstPartyGeneration, first_party_generation_manager};

/// Caller-dependent inputs that affect which skills may be observed.
///
/// None for allowed_upstreams means every configured upstream is route-visible.
/// Some(empty) means first-party only and is the safe default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillCallerScope {
    allowed_upstreams: Option<BTreeSet<String>>,
    subject: Option<String>,
    tool_access: ToolAccess,
}

impl Default for SkillCallerScope {
    fn default() -> Self {
        Self::first_party_only()
    }
}

impl SkillCallerScope {
    #[must_use]
    pub(crate) fn first_party_only() -> Self {
        Self {
            allowed_upstreams: Some(BTreeSet::new()),
            subject: None,
            tool_access: ToolAccess::Direct,
        }
    }

    #[must_use]
    pub(crate) fn root(subject: Option<String>, tool_access: ToolAccess) -> Self {
        Self {
            allowed_upstreams: None,
            subject,
            tool_access,
        }
    }

    #[must_use]
    pub(crate) fn restricted(
        allowed_upstreams: impl IntoIterator<Item = String>,
        subject: Option<String>,
        tool_access: ToolAccess,
    ) -> Self {
        Self {
            allowed_upstreams: Some(allowed_upstreams.into_iter().collect()),
            subject,
            tool_access,
        }
    }

    #[must_use]
    pub(crate) fn allows_upstream(&self, name: &str) -> bool {
        self.allowed_upstreams
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name))
    }

    #[must_use]
    pub(crate) fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    #[must_use]
    pub(crate) const fn tool_access(&self) -> ToolAccess {
        self.tool_access
    }
}

/// Runtime dependencies for canonical Skills operations.
///
/// A missing manager is intentionally first-party-only. The facade never falls
/// back to process-global gateway state because doing so would erase protected
/// route and OAuth-subject boundaries.
pub(crate) struct SkillRegistryContext {
    first_party: Arc<FirstPartyGeneration>,
    #[cfg(feature = "gateway")]
    manager: Option<Arc<GatewayManager>>,
    scope: SkillCallerScope,
    artifact_access: Option<ArtifactAccessSnapshot>,
}

#[derive(Clone)]
pub(crate) struct ArtifactAccessSnapshot {
    tenant_id: LibraryTenantId,
    actor_id: LibraryActorId,
    project_id: LibraryActorId,
    team_ids: BTreeSet<LibraryActorId>,
    is_admin: bool,
    is_platform_admin: bool,
}

impl ArtifactAccessSnapshot {
    pub(crate) fn new(
        tenant_id: LibraryTenantId,
        actor_id: LibraryActorId,
        project_id: LibraryActorId,
        team_ids: BTreeSet<LibraryActorId>,
        is_admin: bool,
        is_platform_admin: bool,
    ) -> Self {
        Self {
            tenant_id,
            actor_id,
            project_id,
            team_ids,
            is_admin,
            is_platform_admin,
        }
    }

    pub(crate) fn permits(
        &self,
        ownership: &labby_runtime::artifacts::LibraryOwnership,
        visibility: SkillVisibility,
    ) -> bool {
        ownership.tenant_id == self.tenant_id
            && match ownership.owner_kind() {
                LibraryOwnerKind::Personal => {
                    ownership.owner_id == self.actor_id
                        || self.is_platform_admin
                        || visibility == SkillVisibility::Tenant
                }
                LibraryOwnerKind::Project => {
                    (ownership.owner_id == self.project_id || self.is_platform_admin)
                        && (visibility == SkillVisibility::Tenant || self.is_admin)
                }
                LibraryOwnerKind::Team => {
                    self.is_platform_admin
                        || (self.team_ids.len() == 1
                            && self.team_ids.contains(&ownership.owner_id)
                            && visibility == SkillVisibility::Tenant)
                }
            }
    }
}

impl SkillRegistryContext {
    #[must_use]
    pub(crate) fn first_party_only() -> Self {
        Self {
            first_party: first_party_generation_manager().generation(),
            #[cfg(feature = "gateway")]
            manager: None,
            scope: SkillCallerScope::first_party_only(),
            artifact_access: None,
        }
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn with_manager(manager: Arc<GatewayManager>, scope: SkillCallerScope) -> Self {
        Self {
            first_party: first_party_generation_manager().generation(),
            manager: Some(manager),
            scope,
            artifact_access: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_generation(first_party: Arc<FirstPartyGeneration>) -> Self {
        Self {
            first_party,
            #[cfg(feature = "gateway")]
            manager: None,
            scope: SkillCallerScope::first_party_only(),
            artifact_access: None,
        }
    }

    #[cfg(all(test, feature = "gateway", feature = "proxy-testkit"))]
    #[must_use]
    pub(crate) fn from_generation_with_manager(
        first_party: Arc<FirstPartyGeneration>,
        manager: Arc<GatewayManager>,
        scope: SkillCallerScope,
    ) -> Self {
        Self {
            first_party,
            manager: Some(manager),
            scope,
            artifact_access: None,
        }
    }

    #[must_use]
    pub(crate) fn generation_id(&self) -> u64 {
        self.first_party.id
    }

    #[must_use]
    pub(crate) fn generation_digest(&self) -> &str {
        &self.first_party.digest
    }

    pub(crate) fn with_artifact_access(mut self, access: ArtifactAccessSnapshot) -> Self {
        self.artifact_access = Some(access);
        self
    }

    fn permits_first_party_uri(&self, uri: &str) -> bool {
        let Some(metadata) = self.first_party.providers.artifact_access(uri) else {
            return true;
        };
        self.artifact_access.as_ref().is_some_and(|access| {
            metadata
                .iter()
                .all(|owner| access.permits(&owner.ownership, owner.visibility))
        })
    }

    fn permits_first_party_entry(&self, entry: &SkillProviderEntry) -> bool {
        self.permits_first_party_uri(entry.descriptor().id.source_id())
            && entry
                .resources()
                .all(|resource| self.permits_first_party_uri(&resource.source_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisibleSkillFile {
    pub(crate) uri: String,
    pub(crate) skill_uri: String,
    pub(crate) origin: String,
    pub(crate) digest: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) content: VisibleSkillContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VisibleSkillContent {
    Text(String),
    Blob(Vec<u8>),
}

impl VisibleSkillFile {
    #[cfg(test)]
    pub(crate) fn text(&self) -> Option<&str> {
        self.content.text()
    }
}

impl VisibleSkillContent {
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Blob(_) => None,
        }
    }

    pub(crate) fn encoded_blob(&self) -> Option<String> {
        match self {
            Self::Text(_) => None,
            Self::Blob(bytes) => Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        }
    }
}

pub(crate) async fn list_visible_skills(context: &SkillRegistryContext) -> SkillsListResult {
    let artifact_entries_filtered = context
        .first_party
        .providers
        .discover()
        .iter()
        .filter(|entry| !context.permits_first_party_entry(entry))
        .count();
    let mut listing = SkillsListResult {
        result_type: Default::default(),
        skills: context
            .first_party
            .providers
            .discover()
            .iter()
            .filter(|entry| context.permits_first_party_entry(entry))
            .cloned()
            .map(provider_entry_to_wire)
            .collect(),
        next_cursor: None,
        // SEP-2640 has no list-changed notification. A generation can refresh,
        // so clients must re-list instead of treating this snapshot as fresh.
        ttl_ms: Some(0),
        cache_scope: Some(
            if context.artifact_access.is_some()
                && context.first_party.providers.has_artifact_skills()
            {
                CACHE_SCOPE_PRIVATE
            } else {
                CACHE_SCOPE_PUBLIC
            }
            .to_string(),
        ),
        meta: None,
    };
    tracing::debug!(
        artifact_entries_filtered,
        "filtered first-party Artifact Skills"
    );

    #[cfg(feature = "gateway")]
    {
        let proxied = proxied_skill_entries(context).await;
        listing.absorb(
            proxied.entries,
            proxied.cache_scope.as_deref(),
            proxied.ttl_ms,
        );
        if proxied.unreachable_upstreams > 0 {
            listing.note_incomplete(
                "unreachableUpstreams",
                serde_json::Value::from(proxied.unreachable_upstreams),
            );
        }
        if proxied.excluded_count > 0 {
            listing.note_incomplete(
                "excludedSkills",
                serde_json::Value::from(proxied.excluded_count),
            );
        }
        if proxied.truncated {
            listing.note_incomplete("truncated", serde_json::Value::Bool(true));
        }
    }

    let _ = context;
    listing
}

pub(crate) async fn get_visible_skill(
    context: &SkillRegistryContext,
    uri: &str,
) -> Result<Option<SkillEntry>, ToolError> {
    let Some(entry) = resolve_visible_skill(context, uri).await? else {
        return Ok(None);
    };
    Ok((entry.uri == uri).then_some(entry))
}

pub(crate) async fn resolve_visible_skill(
    context: &SkillRegistryContext,
    uri: &str,
) -> Result<Option<SkillEntry>, ToolError> {
    if let Some(entry) = context.first_party.providers.find(uri) {
        return Ok(context
            .permits_first_party_entry(entry)
            .then(|| provider_entry_to_wire(entry.clone())));
    }

    // URI validity is part of the Skills contract even when gateway federation
    // is not compiled in. Keep standalone Skills builds aligned with gateway
    // builds instead of treating malformed identifiers as missing resources.
    let parsed = parse_skill_uri(uri).map_err(|error| ToolError::InvalidParam {
        message: error.to_string(),
        param: "uri".to_string(),
    })?;

    #[cfg(feature = "gateway")]
    {
        let origin = parsed.origin().to_string();
        if !context.scope.allows_upstream(&origin) {
            return Ok(None);
        }
        let Some(manager) = context.manager.as_deref() else {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_unavailable".into(),
                message: "gateway runtime is unavailable while resolving a skill".into(),
            });
        };
        let Some(config) = manager.upstream_config(&origin).await else {
            return Ok(None);
        };
        if !config.enabled || !config.proxy_skills {
            return Ok(None);
        }
        let pool = manager.current_pool().await.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "upstream_unavailable".into(),
            message: "gateway runtime is unavailable while resolving a skill".into(),
        })?;
        let provider = SepSkillProvider::new(
            Arc::clone(&pool),
            config.clone(),
            context.scope.subject().map(str::to_string),
        );
        let discovered = provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .map_err(provider_error_to_tool)?;
        let validated = discovered
            .skills
            .into_iter()
            .map(SkillProviderEntry::into_validated)
            .collect::<Vec<_>>();
        let meta = origin_meta(&origin, &pool, context.scope.tool_access()).await;
        let minted = aggregate::mint_proxied_entries(&config, &validated, Some(&meta));
        if let Some(entry) = minted.entries.iter().find(|entry| entry.uri == uri) {
            return Ok(Some(entry.clone()));
        }

        // A URI already owned by a collision-excluded skill stays poisoned.
        // Do not let an inconsistent `skills/get` response resurrect it.
        if minted.excludes_uri(uri) {
            return Ok(None);
        }

        let Some(upstream_uri) = parsed.upstream_uri_for_origin(&config.name) else {
            return Ok(None);
        };
        if let Some(cached) = provider.cached_owner_for_resource(&upstream_uri).await {
            let Some(entry) =
                aggregate::mint_proxied_entry(&config.name, cached.validated(), Some(&meta))
            else {
                return Ok(None);
            };
            if !minted.conflicts_with(&entry) {
                return Ok(Some(entry));
            }
            return Ok(None);
        }
        let upstream_skill_uri = labby_runtime::skills::parse_skill_resource_uri(&upstream_uri)
            .map_err(|error| ToolError::InvalidParam {
                message: error.to_string(),
                param: "uri".to_string(),
            })?;
        if upstream_skill_uri.skill_md_parts().is_none() {
            return Ok(None);
        }
        let fetched = match provider
            .get(&SkillGetRequest {
                id: SkillId::new(provider.id().clone(), upstream_uri),
                deadline: SkillProviderDeadline::default(),
            })
            .await
        {
            Ok(result) => result.skill.into_validated(),
            Err(SkillProviderError::SkillNotFound) => return Ok(None),
            Err(error) => return Err(provider_error_to_tool(error)),
        };
        let Some(entry) = aggregate::mint_proxied_entry(&config.name, &fetched, Some(&meta)) else {
            return Ok(None);
        };
        if minted.conflicts_with(&entry) {
            tracing::warn!(
                upstream = %config.name,
                skill = %entry.uri,
                "excluding unlisted skill whose manifest collides with published URI ownership"
            );
            return Ok(None);
        }
        return Ok(Some(entry));
    }

    #[cfg(not(feature = "gateway"))]
    {
        drop(parsed);
        Ok(None)
    }
}

pub(crate) async fn read_visible_skill_file(
    context: &SkillRegistryContext,
    uri: &str,
) -> Result<VisibleSkillFile, ToolError> {
    let first_party_owners = context.first_party.providers.find_all(uri);
    if let Some(provider_entry) = first_party_owners.first().copied() {
        if !context.permits_first_party_uri(uri) {
            return Err(unknown_file(uri));
        }
        let entry = &provider_entry.validated().entry;
        let resource = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == uri))
            .cloned()
            .ok_or_else(|| stale_manifest(uri))?;
        if first_party_owners.iter().skip(1).any(|owner| {
            owner
                .validated()
                .entry
                .resources
                .as_ref()
                .and_then(|resources| resources.iter().find(|candidate| candidate.uri == uri))
                != Some(&resource)
        }) {
            return Err(stale_manifest(uri));
        }
        let verified = context
            .first_party
            .providers
            .read(&provider_entry, uri, limits::MAX_SKILL_RESOURCE_BYTES)
            .await
            .map_err(first_party_provider_error_to_tool)?;
        let content = match String::from_utf8(verified.bytes) {
            Ok(text) => VisibleSkillContent::Text(text),
            Err(error) => VisibleSkillContent::Blob(error.into_bytes()),
        };
        return Ok(VisibleSkillFile {
            uri: uri.to_string(),
            skill_uri: entry.uri.clone(),
            origin: labby_runtime::skills::FIRST_PARTY_ORIGIN.to_string(),
            digest: resource.digest,
            mime_type: (entry.uri == uri)
                .then(|| labby_runtime::skills::SKILL_MD_MIME_TYPE.to_string()),
            content,
        });
    }

    #[cfg(feature = "gateway")]
    {
        let mut owners = list_visible_skills(context)
            .await
            .skills
            .into_iter()
            .filter(|entry| {
                entry
                    .resources
                    .as_ref()
                    .is_some_and(|resources| resources.iter().any(|resource| resource.uri == uri))
            })
            .collect::<Vec<_>>();
        if owners.is_empty()
            && let Some(owner) = resolve_visible_skill(context, uri).await?
        {
            owners.push(owner);
        }
        let entry = owners.first().cloned().ok_or_else(|| unknown_file(uri))?;
        let expected_resource = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == uri))
            .ok_or_else(|| stale_manifest(uri))?;
        if owners.iter().skip(1).any(|owner| {
            owner
                .resources
                .as_ref()
                .and_then(|resources| resources.iter().find(|resource| resource.uri == uri))
                != Some(expected_resource)
        }) {
            return Err(stale_manifest(uri));
        }
        let resource = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == uri))
            .cloned()
            .ok_or_else(|| stale_manifest(uri))?;
        let parsed = parse_skill_uri(uri).map_err(|error| ToolError::InvalidParam {
            message: error.to_string(),
            param: "uri".to_string(),
        })?;
        let origin = parsed.origin().to_string();
        if !context.scope.allows_upstream(&origin) {
            return Err(unknown_file(uri));
        }
        let manager = context
            .manager
            .as_deref()
            .ok_or_else(|| unknown_file(uri))?;
        let config = manager
            .upstream_config(&origin)
            .await
            .filter(|config| config.enabled && config.proxy_skills)
            .ok_or_else(|| unknown_file(uri))?;
        let pool = manager.current_pool().await.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "upstream_unavailable".to_string(),
            message: "gateway runtime is unavailable while reading a skill file".to_string(),
        })?;
        let upstream_uri = parsed
            .upstream_uri_for_origin(&origin)
            .ok_or_else(|| unknown_file(uri))?;
        let provider =
            SepSkillProvider::new(pool, config, context.scope.subject().map(str::to_string));
        let skill_source_id = parse_skill_uri(&entry.uri)
            .ok()
            .and_then(|uri| uri.upstream_uri_for_origin(&origin))
            .ok_or_else(|| stale_manifest(uri))?;
        let verified = provider
            .read_resource(&SkillResourceReadRequest {
                skill_id: SkillId::new(provider.id().clone(), skill_source_id),
                resource_id: upstream_uri,
                max_bytes: limits::MAX_SKILL_RESOURCE_BYTES,
                deadline: SkillProviderDeadline::default(),
            })
            .await
            .map_err(provider_error_to_tool)?;
        let content = match verified.representation {
            labby_runtime::skills::SkillResourceRepresentation::Text => {
                let text = String::from_utf8(verified.bytes).map_err(|_| ToolError::Sdk {
                    sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                    message: "verified MCP text skill resource was not UTF-8".into(),
                })?;
                VisibleSkillContent::Text(text)
            }
            labby_runtime::skills::SkillResourceRepresentation::Blob => {
                VisibleSkillContent::Blob(verified.bytes)
            }
        };
        let is_skill_md = entry.uri == uri;
        return Ok(VisibleSkillFile {
            uri: uri.to_string(),
            skill_uri: entry.uri,
            origin,
            digest: resource.digest,
            mime_type: if is_skill_md {
                Some(labby_runtime::skills::SKILL_MD_MIME_TYPE.to_string())
            } else {
                verified.media_type
            },
            content,
        });
    }

    #[cfg(not(feature = "gateway"))]
    {
        let _ = context;
        Err(unknown_file(uri))
    }
}

fn provider_entry_to_wire(skill: SkillProviderEntry) -> SkillEntry {
    skill.into_validated().entry
}

fn first_party_provider_error_to_tool(error: SkillProviderError) -> ToolError {
    provider_error_with_failure_kind(error, "provider_error")
}

fn provider_error_with_failure_kind(
    error: SkillProviderError,
    provider_failure_kind: &'static str,
) -> ToolError {
    let sdk_kind = match error {
        SkillProviderError::InvalidRequest { .. } | SkillProviderError::WrongProvider => {
            "invalid_param"
        }
        SkillProviderError::SkillNotFound | SkillProviderError::ResourceNotFound => "not_found",
        SkillProviderError::ManifestStale => labby_runtime::skills::KIND_SKILL_MANIFEST_STALE,
        SkillProviderError::DeadlineExceeded => "timeout",
        SkillProviderError::LimitExceeded { .. } => "response_too_large",
        SkillProviderError::Integrity { .. } => labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH,
        SkillProviderError::Unavailable { .. } | SkillProviderError::Provider { .. } => {
            provider_failure_kind
        }
    };
    ToolError::Sdk {
        sdk_kind: sdk_kind.to_string(),
        message: error.to_string(),
    }
}

fn unknown_file(uri: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("'{uri}' is not a skill file this caller can access"),
    }
}

fn stale_manifest(uri: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: labby_runtime::skills::KIND_SKILL_MANIFEST_STALE.to_string(),
        message: format!("the current skill manifest does not bind '{uri}'"),
    }
}

#[cfg(feature = "gateway")]
#[derive(Debug, Default)]
struct ProxiedSkills {
    entries: Vec<SkillEntry>,
    excluded_uris: BTreeSet<String>,
    unreachable_upstreams: usize,
    excluded_count: usize,
    truncated: bool,
    cache_scope: Option<String>,
    ttl_ms: Option<u64>,
}

#[cfg(feature = "gateway")]
fn unavailable_proxied_skills(upstream_count: usize) -> ProxiedSkills {
    ProxiedSkills {
        unreachable_upstreams: upstream_count,
        cache_scope: (upstream_count > 0).then(|| CACHE_SCOPE_PRIVATE.to_string()),
        ..ProxiedSkills::default()
    }
}

#[cfg(feature = "gateway")]
async fn proxied_skill_entries(context: &SkillRegistryContext) -> ProxiedSkills {
    let Some(manager) = context.manager.as_deref() else {
        return ProxiedSkills::default();
    };
    let configs = manager
        .current_config()
        .await
        .upstream
        .into_iter()
        .filter(|config| config.enabled && config.proxy_skills)
        .filter(|config| context.scope.allows_upstream(&config.name))
        .collect::<Vec<_>>();
    let Some(pool) = manager.current_pool().await else {
        if !configs.is_empty() {
            tracing::warn!(
                surface = "dispatch",
                service = "skills",
                upstream_count = configs.len(),
                "gateway runtime unavailable while listing configured Skill upstreams"
            );
        }
        return unavailable_proxied_skills(configs.len());
    };

    let subject = context.scope.subject().map(str::to_string);
    let mut results = stream::iter(configs)
        .map(|config| {
            let pool = Arc::clone(&pool);
            let subject = subject.clone();
            async move {
                let provider = SepSkillProvider::new(Arc::clone(&pool), config.clone(), subject);
                let result = provider.discover(&SkillDiscoverRequest::default()).await;
                (config, result)
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    results.sort_by(|(left, _), (right, _)| left.name.cmp(&right.name));

    let mut aggregated = ProxiedSkills::default();
    if !results.is_empty() {
        aggregated.cache_scope = Some(CACHE_SCOPE_PRIVATE.to_string());
    }
    for (config, result) in results {
        match result {
            Ok(discovered) => {
                aggregated.excluded_count += discovered.excluded_count;
                aggregated.truncated |= discovered.truncated;
                let ttl_ms = discovered
                    .ttl
                    .and_then(|ttl| u64::try_from(ttl.as_millis()).ok());
                aggregated.ttl_ms = min_ttl(aggregated.ttl_ms, ttl_ms);
                let meta = origin_meta(&config.name, &pool, context.scope.tool_access()).await;
                let validated = discovered
                    .skills
                    .into_iter()
                    .map(SkillProviderEntry::into_validated)
                    .collect::<Vec<_>>();
                let minted = aggregate::mint_proxied_entries(&config, &validated, Some(&meta));
                aggregated.excluded_count += minted.excluded_count;
                aggregated.excluded_uris.extend(minted.excluded_uris);
                aggregated.entries.extend(minted.entries);
            }
            Err(error) => {
                aggregated.unreachable_upstreams += 1;
                tracing::warn!(
                    upstream = %config.name,
                    error = %error,
                    "skipping an upstream while aggregating skills"
                );
            }
        }
    }
    aggregated
}

#[cfg(feature = "gateway")]
fn provider_error_to_tool(error: SkillProviderError) -> ToolError {
    provider_error_with_failure_kind(error, "upstream_error")
}

#[cfg(feature = "gateway")]
async fn origin_meta(
    origin: &str,
    pool: &UpstreamPool,
    access: ToolAccess,
) -> serde_json::Map<String, serde_json::Value> {
    let reachable = if access == ToolAccess::Direct {
        pool.healthy_tools_for_upstream(origin)
            .await
            .into_iter()
            .map(|tool| tool.tool.name.to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    aggregate::origin_meta(origin, access, &reachable)
}

#[cfg(feature = "gateway")]
fn min_ttl(current: Option<u64>, incoming: Option<u64>) -> Option<u64> {
    match (current, incoming) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::local::LocalSkill;
    use crate::skills::providers::{ArtifactSkillAccess, FirstPartySkillProviders};
    use labby_runtime::artifacts::LibraryOwnership;
    use labby_runtime::skills::wire::SkillResource;

    #[cfg(feature = "gateway")]
    #[test]
    fn unavailable_pool_marks_every_configured_upstream_incomplete() {
        let proxied = unavailable_proxied_skills(2);
        assert_eq!(proxied.unreachable_upstreams, 2);
        assert_eq!(proxied.cache_scope.as_deref(), Some(CACHE_SCOPE_PRIVATE));
        assert!(proxied.entries.is_empty());
    }

    #[tokio::test]
    async fn resolution_preserves_invalid_uri_errors() {
        let context = SkillRegistryContext::first_party_only();
        let invalid = resolve_visible_skill(&context, "not a skill uri")
            .await
            .expect_err("malformed identifiers are not reported as absence");
        assert!(matches!(invalid, ToolError::InvalidParam { .. }));
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn resolution_preserves_missing_runtime_errors() {
        let mut root_context = SkillRegistryContext::first_party_only();
        root_context.scope = SkillCallerScope::root(None, ToolAccess::Direct);
        let unavailable = resolve_visible_skill(&root_context, "skill://remote/demo/SKILL.md")
            .await
            .expect_err("a missing gateway runtime is not reported as absence");
        assert_eq!(unavailable.kind(), "upstream_unavailable");
    }

    fn artifact_context(visibility: SkillVisibility) -> SkillRegistryContext {
        use std::collections::BTreeMap;

        use crate::skills::registry::FirstPartyGeneration;
        use labby_runtime::artifacts::LibraryOwnership;
        use labby_runtime::skills::ResourceDigest;
        use labby_runtime::skills::wire::SkillEntry;

        let manifest = "skill://labby/artifact/SKILL.md";
        let support = "skill://labby/artifact/notes.md";
        let body = "---\nname: artifact\ndescription: private\n---\n\nbody\n";
        let notes = "owner notes";
        let skill = LocalSkill {
            entry: SkillEntry {
                uri: manifest.to_owned(),
                frontmatter: labby_runtime::skills::parse_skill_md_frontmatter(body).unwrap(),
                resources: Some(vec![
                    SkillResource {
                        uri: manifest.to_owned(),
                        digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
                        size: body.len() as u64,
                    },
                    SkillResource {
                        uri: support.to_owned(),
                        digest: ResourceDigest::of_bytes(notes.as_bytes()).to_wire(),
                        size: notes.len() as u64,
                    },
                ]),
                meta: None,
            },
            files: BTreeMap::from([
                (manifest.to_owned(), body.to_owned()),
                (support.to_owned(), notes.to_owned()),
            ]),
        };
        let providers = FirstPartySkillProviders::from_artifact_skills([(
            skill,
            ArtifactSkillAccess {
                ownership: LibraryOwnership::canonical(
                    LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
                    LibraryActorId::from_canonical_projection("owner").unwrap(),
                ),
                visibility,
            },
        )]);
        SkillRegistryContext::from_generation(Arc::new(FirstPartyGeneration {
            id: 7,
            digest: "digest".to_owned(),
            active_digest: "active".to_owned(),
            providers,
            rejected: Vec::new(),
            bytes: body.len() + notes.len(),
            resources: 2,
            degraded: None,
        }))
    }

    fn artifact_access_with_platform_role(
        tenant: &str,
        actor: &str,
        is_admin: bool,
        is_platform_admin: bool,
    ) -> ArtifactAccessSnapshot {
        ArtifactAccessSnapshot::new(
            LibraryTenantId::from_canonical_projection(tenant).unwrap(),
            LibraryActorId::from_canonical_projection(actor).unwrap(),
            LibraryActorId::from_canonical_projection("project").unwrap(),
            BTreeSet::new(),
            is_admin,
            is_platform_admin,
        )
    }

    fn artifact_access(tenant: &str, actor: &str, is_admin: bool) -> ArtifactAccessSnapshot {
        artifact_access_with_platform_role(tenant, actor, is_admin, false)
    }

    use labby_runtime::skills::ResourceDigest;

    #[test]
    fn default_scope_is_first_party_only() {
        let scope = SkillCallerScope::default();
        assert!(!scope.allows_upstream("github"));
        assert!(scope.subject().is_none());
    }

    #[test]
    fn root_scope_allows_every_upstream() {
        let scope = SkillCallerScope::root(Some("alice".to_string()), ToolAccess::Direct);
        assert!(scope.allows_upstream("github"));
        assert!(scope.allows_upstream("gitlab"));
        assert_eq!(scope.subject(), Some("alice"));
    }

    #[test]
    fn protected_scope_is_an_allowlist() {
        let scope = SkillCallerScope::restricted(
            ["github".to_string(), "docs".to_string()],
            None,
            ToolAccess::CodeModeOnly,
        );
        assert!(scope.allows_upstream("github"));
        assert!(scope.allows_upstream("docs"));
        assert!(!scope.allows_upstream("private"));
        assert_eq!(scope.tool_access(), ToolAccess::CodeModeOnly);
    }

    #[tokio::test]
    async fn first_party_context_lists_and_reads_same_registry() {
        let context = SkillRegistryContext::first_party_only();
        let listing = list_visible_skills(&context).await;
        let entry = listing
            .skills
            .iter()
            .find(|entry| entry.uri == "skill://labby/using-labby/SKILL.md")
            .expect("bundled skill");
        let file = read_visible_skill_file(&context, &entry.uri)
            .await
            .expect("read");
        assert_eq!(file.skill_uri, entry.uri);
        assert!(file.text().unwrap().contains("name: using-labby"));
        let digest = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == entry.uri))
            .expect("SKILL.md digest");
        assert_eq!(file.digest, digest.digest);
        assert_eq!(listing.ttl_ms, Some(0));
    }

    #[tokio::test]
    async fn artifact_visibility_filters_manifest_and_unlisted_support_uri() {
        let private = artifact_context(SkillVisibility::Private);
        assert!(
            get_visible_skill(&private, "skill://labby/using-labby/SKILL.md")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            get_visible_skill(&private, "skill://labby/artifact/SKILL.md")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            get_visible_skill(&private, "skill://labby/artifact/notes.md")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            read_visible_skill_file(&private, "skill://labby/artifact/notes.md")
                .await
                .is_err()
        );

        let member = artifact_context(SkillVisibility::Private)
            .with_artifact_access(artifact_access("tenant-a", "member", false));
        assert!(
            get_visible_skill(&member, "skill://labby/artifact/SKILL.md")
                .await
                .unwrap()
                .is_none()
        );

        let owner = artifact_context(SkillVisibility::Private)
            .with_artifact_access(artifact_access("tenant-a", "owner", false));
        assert_eq!(
            read_visible_skill_file(&owner, "skill://labby/artifact/notes.md")
                .await
                .unwrap()
                .text(),
            Some("owner notes")
        );
        assert_eq!(
            list_visible_skills(&owner).await.cache_scope.as_deref(),
            Some(CACHE_SCOPE_PRIVATE)
        );

        let admin = artifact_context(SkillVisibility::Private)
            .with_artifact_access(artifact_access("tenant-a", "admin", true));
        assert!(
            resolve_visible_skill(&admin, "skill://labby/artifact/notes.md")
                .await
                .unwrap()
                .is_none(),
            "project administration does not grant access to another principal's private records"
        );

        let platform_admin = artifact_context(SkillVisibility::Private).with_artifact_access(
            artifact_access_with_platform_role("tenant-a", "platform-admin", true, true),
        );
        assert!(
            resolve_visible_skill(&platform_admin, "skill://labby/artifact/notes.md")
                .await
                .unwrap()
                .is_some()
        );

        let tenant_member = artifact_context(SkillVisibility::Tenant)
            .with_artifact_access(artifact_access("tenant-a", "member", false));
        assert!(
            resolve_visible_skill(&tenant_member, "skill://labby/artifact/notes.md")
                .await
                .unwrap()
                .is_some()
        );

        let cross_tenant = artifact_context(SkillVisibility::Tenant)
            .with_artifact_access(artifact_access("tenant-b", "owner", true));
        assert!(
            get_visible_skill(&cross_tenant, "skill://labby/artifact/notes.md")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn overlapping_artifact_resource_requires_access_to_every_owner() {
        use std::collections::BTreeMap;

        let shared_uri = "skill://labby/parent/child/shared.txt";
        let shared = "shared bytes";
        let make_skill = |name: &str, manifest: &str| {
            let body = format!("---\nname: {name}\ndescription: nested\n---\n");
            LocalSkill {
                entry: SkillEntry {
                    uri: manifest.to_owned(),
                    frontmatter: labby_runtime::skills::parse_skill_md_frontmatter(&body).unwrap(),
                    resources: Some(vec![
                        SkillResource {
                            uri: manifest.to_owned(),
                            digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
                            size: body.len() as u64,
                        },
                        SkillResource {
                            uri: shared_uri.to_owned(),
                            digest: ResourceDigest::of_bytes(shared.as_bytes()).to_wire(),
                            size: shared.len() as u64,
                        },
                    ]),
                    meta: None,
                },
                files: BTreeMap::from([
                    (manifest.to_owned(), body),
                    (shared_uri.to_owned(), shared.to_owned()),
                ]),
            }
        };
        let ownership = |actor: &str| {
            LibraryOwnership::canonical(
                LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
                LibraryActorId::from_canonical_projection(actor).unwrap(),
            )
        };
        let providers = FirstPartySkillProviders::from_artifact_skills([
            (
                make_skill("parent", "skill://labby/parent/SKILL.md"),
                ArtifactSkillAccess {
                    ownership: ownership("owner"),
                    visibility: SkillVisibility::Private,
                },
            ),
            (
                make_skill("child", "skill://labby/parent/child/SKILL.md"),
                ArtifactSkillAccess {
                    ownership: ownership("child-owner"),
                    visibility: SkillVisibility::Tenant,
                },
            ),
        ]);
        assert_eq!(providers.find_all(shared_uri).len(), 2);
        assert_eq!(providers.artifact_access(shared_uri).unwrap().len(), 2);
        let generation = Arc::new(FirstPartyGeneration {
            id: 8,
            digest: "nested".into(),
            active_digest: "nested-active".into(),
            providers,
            rejected: Vec::new(),
            bytes: shared.len(),
            resources: 3,
            degraded: None,
        });
        let member = SkillRegistryContext::from_generation(Arc::clone(&generation))
            .with_artifact_access(artifact_access("tenant-a", "member", false));
        let member_listing = list_visible_skills(&member).await;
        assert!(
            member_listing
                .skills
                .iter()
                .all(|entry| entry.uri != "skill://labby/parent/child/SKILL.md"),
            "a caller must not discover a skill whose complete resource set is unreadable"
        );
        assert!(read_visible_skill_file(&member, shared_uri).await.is_err());
        let owner = SkillRegistryContext::from_generation(generation)
            .with_artifact_access(artifact_access("tenant-a", "owner", false));
        assert_eq!(
            read_visible_skill_file(&owner, shared_uri)
                .await
                .unwrap()
                .text(),
            Some(shared)
        );
    }

    #[tokio::test]
    async fn supporting_file_and_manifest_remain_on_the_captured_generation() {
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("pinned");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: pinned\ndescription: old\n---\n\nold\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), "old notes").unwrap();
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let pinned = SkillRegistryContext::from_generation(manager.generation());
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: pinned\ndescription: new\n---\n\nnew\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), "new notes").unwrap();
        manager.refresh(None).unwrap();

        let notes_uri = "skill://labby/pinned/notes.md";
        let old_entry = resolve_visible_skill(&pinned, notes_uri)
            .await
            .unwrap()
            .unwrap();
        let old_file = read_visible_skill_file(&pinned, notes_uri).await.unwrap();
        assert_eq!(old_entry.frontmatter["description"], "old");
        assert_eq!(old_file.text(), Some("old notes"));
        let resource = old_entry
            .resources
            .as_ref()
            .unwrap()
            .iter()
            .find(|resource| resource.uri == notes_uri)
            .unwrap();
        assert_eq!(resource.digest, old_file.digest);
        assert!(
            labby_runtime::skills::parse_digest(&resource.digest)
                .unwrap()
                .matches(old_file.text().unwrap().as_bytes())
        );
    }

    #[tokio::test]
    #[cfg(all(feature = "gateway", feature = "skills", feature = "proxy-testkit"))]
    async fn reminted_unlisted_supporting_uri_resolves_and_reads_through_gateway() {
        use std::collections::HashMap;

        use labby_gateway::gateway::manager::GatewayRuntimeHandle;
        use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};
        use serde_json::json;

        let skill_body = "---\nname: unlisted\ndescription: a test skill\n---\n\n# Body\n";
        let native_skill_uri = "skill://native/unlisted/SKILL.md";
        let native_notes_uri = "skill://native/unlisted/notes.md";
        let reminted_skill_uri = "skill://up/skill/native/unlisted/SKILL.md";
        let reminted_notes_uri = "skill://up/skill/native/unlisted/notes.md";
        let notes_digest = ResourceDigest::of_bytes(b"supporting notes").to_wire();
        let parent_body = "---\nname: parent\ndescription: parent skill\n---\n";
        let child_body = "---\nname: child\ndescription: child skill\n---\n";
        let parent_uri = "skill://native/parent/SKILL.md";
        let child_uri = "skill://native/parent/child/SKILL.md";
        let shared_uri = "skill://native/parent/child/shared.txt";
        let shared_body = "shared supporting bytes";
        let shared_resource = json!({
            "uri": shared_uri,
            "digest": ResourceDigest::of_bytes(shared_body.as_bytes()).to_wire(),
            "size": shared_body.len()
        });
        let nested_listing = json!({
            "resultType": "complete",
            "skills": [
                {
                    "uri": parent_uri,
                    "frontmatter": { "name": "parent", "description": "parent skill" },
                    "resources": [
                        { "uri": parent_uri, "digest": ResourceDigest::of_bytes(parent_body.as_bytes()).to_wire(), "size": parent_body.len() },
                        shared_resource.clone()
                    ]
                },
                {
                    "uri": child_uri,
                    "frontmatter": { "name": "child", "description": "child skill" },
                    "resources": [
                        { "uri": child_uri, "digest": ResourceDigest::of_bytes(child_body.as_bytes()).to_wire(), "size": child_body.len() },
                        shared_resource
                    ]
                }
            ]
        });
        let unlisted_entry = json!({
            "uri": native_skill_uri,
            "frontmatter": { "name": "unlisted", "description": "a test skill" },
            "resources": [
                {
                    "uri": native_skill_uri,
                    "digest": ResourceDigest::of_bytes(skill_body.as_bytes()).to_wire(),
                    "size": skill_body.len()
                },
                { "uri": native_notes_uri, "digest": notes_digest, "size": "supporting notes".len() }
            ]
        });

        let pool = Arc::new(UpstreamPool::new());
        pool.insert_scripted_skills_server_for_tests(
            "up",
            nested_listing,
            unlisted_entry,
            HashMap::from([
                (native_skill_uri.to_string(), skill_body.to_string()),
                (native_notes_uri.to_string(), "supporting notes".to_string()),
                (parent_uri.to_string(), parent_body.to_string()),
                (child_uri.to_string(), child_body.to_string()),
                (shared_uri.to_string(), shared_body.to_string()),
            ]),
        )
        .await;

        let upstream = UpstreamConfig {
            enabled: true,
            name: "up".to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: Default::default(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: true,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        let runtime = GatewayRuntimeHandle::default();
        runtime.swap(Some(pool)).await;
        let gateway_manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        gateway_manager
            .seed_config_unchecked_for_tests(GatewayConfig {
                upstream: vec![upstream],
                ..GatewayConfig::default()
            })
            .await;
        let generation_root = tempfile::tempdir().unwrap();
        let generation_skill = generation_root.path().join("generation-marker");
        std::fs::create_dir_all(&generation_skill).unwrap();
        std::fs::write(
            generation_skill.join("SKILL.md"),
            "---\nname: generation-marker\ndescription: old\n---\n",
        )
        .unwrap();
        let generation_manager = crate::skills::registry::FirstPartyGenerationManager::new(
            generation_root.path().to_path_buf(),
            crate::skills::registry::GenerationLimits::default(),
        );
        let pinned_generation = generation_manager.generation();
        let pinned_id = pinned_generation.id;
        let context = SkillRegistryContext::from_generation_with_manager(
            pinned_generation,
            gateway_manager,
            SkillCallerScope::root(Some("alice".to_string()), ToolAccess::Direct),
        );
        std::fs::write(
            generation_skill.join("SKILL.md"),
            "---\nname: generation-marker\ndescription: new\n---\n",
        )
        .unwrap();
        generation_manager.refresh(None).unwrap();
        assert_ne!(pinned_id, generation_manager.generation().id);
        assert_eq!(context.generation_id(), pinned_id);

        let listing = list_visible_skills(&context).await;
        assert!(
            listing
                .skills
                .iter()
                .any(|entry| entry.uri.ends_with("/parent/SKILL.md"))
        );
        assert!(
            listing
                .skills
                .iter()
                .any(|entry| entry.uri.ends_with("/parent/child/SKILL.md"))
        );
        let reminted_shared_uri = "skill://up/skill/native/parent/child/shared.txt";
        let shared_file = read_visible_skill_file(&context, reminted_shared_uri)
            .await
            .expect("identically bound nested supporting resource remains readable");
        assert_eq!(shared_file.text(), Some(shared_body));

        let fetched = get_visible_skill(&context, reminted_skill_uri)
            .await
            .unwrap()
            .expect("unlisted skill resolves through skills/get");
        assert_eq!(fetched.uri, reminted_skill_uri);

        let entry = resolve_visible_skill(&context, reminted_notes_uri)
            .await
            .unwrap()
            .expect("unlisted supporting URI resolves through cached ownership");
        assert_eq!(entry.uri, reminted_skill_uri);
        assert!(entry.resources.as_ref().is_some_and(|resources| {
            resources
                .iter()
                .any(|resource| resource.uri == reminted_notes_uri)
        }));

        let file = read_visible_skill_file(&context, reminted_notes_uri)
            .await
            .expect("cached owner binds the supporting resource read");
        assert_eq!(file.uri, reminted_notes_uri);
        assert_eq!(file.skill_uri, reminted_skill_uri);
        assert_eq!(file.origin, "up");
        assert_eq!(file.digest, notes_digest);
        assert_eq!(file.text(), Some("supporting notes"));
    }
}
