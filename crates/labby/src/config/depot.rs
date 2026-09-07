//! Product-local discovery configuration. Resolution performs no I/O.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const PUBLIC_ID: &str = "public";
pub const LEGACY_ID: &str = "legacy";
pub const PUBLIC_ENDPOINT: &str = "https://depot.dinglebear.ai";
pub const MAX_PROVIDERS: usize = 16;
pub const MAX_TOMBSTONES: usize = 4096;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepotControlMode {
    #[default]
    Standalone,
    LabbyManaged,
}

/// Wire generations remain opaque strings even if an upstream uses numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpaqueEpoch(String);

impl TryFrom<String> for OpaqueEpoch {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !(1..=128).contains(&value.chars().count()) {
            return Err("invalid_epoch");
        }
        Ok(Self(value))
    }
}

impl From<OpaqueEpoch> for String {
    fn from(value: OpaqueEpoch) -> Self {
        value.0
    }
}

/// Raw entries retain malformed siblings and future fields for targeted edits.
/// Never use this disk model as an HTTP response; use `ResolvedDepot` instead.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DepotPreferences {
    pub control_mode: DepotControlMode,
    pub managed_authority_kill_switch: bool,
    pub public_enabled: bool,
    pub providers: Vec<toml::Value>,
    pub tombstones: BTreeSet<String>,
    pub legacy_migrated: bool,
    /// Managed authority replication target and secret references. The signing
    /// key and bearer value are resolved from the named environment variables
    /// only when the daemon starts; they are never serialized into projections.
    pub authority_endpoint: Option<String>,
    pub authority_bearer_token_env: Option<String>,
    pub authority_installation_id: Option<String>,
    pub authority_key_id: Option<String>,
    pub authority_signing_key_env: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Default for DepotPreferences {
    fn default() -> Self {
        Self {
            control_mode: DepotControlMode::Standalone,
            managed_authority_kill_switch: false,
            public_enabled: true,
            providers: Vec::new(),
            tombstones: BTreeSet::new(),
            legacy_migrated: false,
            authority_endpoint: None,
            authority_bearer_token_env: None,
            authority_installation_id: None,
            authority_key_id: None,
            authority_signing_key_env: None,
            extra: BTreeMap::new(),
        }
    }
}

impl std::fmt::Debug for DepotPreferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepotPreferences")
            .field("public_enabled", &self.public_enabled)
            .field("control_mode", &self.control_mode)
            .field(
                "managed_authority_kill_switch",
                &self.managed_authority_kill_switch,
            )
            .field("provider_count", &self.providers.len())
            .field("tombstone_count", &self.tombstones.len())
            .field("legacy_migrated", &self.legacy_migrated)
            .field(
                "authority_endpoint_configured",
                &self.authority_endpoint.is_some(),
            )
            .field(
                "authority_credentials_configured",
                &(self.authority_bearer_token_env.is_some()
                    && self.authority_signing_key_env.is_some()),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Anonymous,
    Bearer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub auth_mode: AuthMode,
    pub bearer_token_env: Option<String>,
}

/// Safe configuration projection. Credentials and raw diagnostics never cross
/// the surface boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub auth_mode: AuthMode,
    #[serde(skip)]
    pub bearer_token_env: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDiagnostic {
    pub entry_index: usize,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedDepot {
    pub providers: Vec<ProviderView>,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

/// Only presence is needed during normalization. Secret values are resolved
/// later from an immutable server-held snapshot.
#[derive(Debug, Clone, Default)]
pub struct LegacyDepot {
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub token_present: bool,
}

impl DepotPreferences {
    /// Managed mode must never fall back to standalone authority when its
    /// projection path is stale, disabled, or on an unknown protocol version.
    #[must_use]
    pub fn managed_mutations_ready(&self, projection_ready: bool, protocol_version: u64) -> bool {
        self.control_mode == DepotControlMode::LabbyManaged
            && !self.managed_authority_kill_switch
            && projection_ready
            && protocol_version == 1
    }

    #[must_use]
    pub fn resolve(&self, legacy: &LegacyDepot) -> ResolvedDepot {
        let mut result = ResolvedDepot {
            providers: vec![ProviderView {
                id: PUBLIC_ID.into(),
                name: "Public Depot".into(),
                endpoint: PUBLIC_ENDPOINT.into(),
                enabled: self.public_enabled,
                auth_mode: AuthMode::Anonymous,
                bearer_token_env: None,
            }],
            diagnostics: Vec::new(),
        };
        if self.tombstones.len() > MAX_TOMBSTONES {
            result.diagnostics.push(ConfigDiagnostic {
                entry_index: 0,
                kind: "tombstone_capacity",
            });
            return result;
        }
        // Count IDs before selecting any entry, including malformed siblings.
        let mut counts = BTreeMap::<&str, usize>::new();
        for raw in &self.providers {
            if let Some(id) = raw.get("id").and_then(toml::Value::as_str) {
                *counts.entry(id).or_default() += 1;
            }
        }
        let pending_legacy = !self.legacy_migrated
            && !self.tombstones.contains(LEGACY_ID)
            && (legacy.url.is_some() || legacy.enabled.is_some() || legacy.token_present);
        let slots = MAX_PROVIDERS - 1 - usize::from(pending_legacy);
        for (index, raw) in self.providers.iter().take(MAX_PROVIDERS).enumerate() {
            let parsed = raw.clone().try_into::<ProviderConfig>();
            let checked = parsed.map_err(|_| "invalid_entry").and_then(|provider| {
                if index >= slots {
                    return Err("provider_capacity");
                }
                if counts.get(provider.id.as_str()).copied().unwrap_or(0) != 1 {
                    return Err("duplicate_id");
                }
                if provider.id == PUBLIC_ID
                    || provider.id == "all"
                    || (provider.id == LEGACY_ID && !self.legacy_migrated)
                {
                    return Err("reserved_id");
                }
                if self.tombstones.contains(&provider.id) {
                    return Err("removed_id");
                }
                provider.validate()?;
                Ok(provider)
            });
            match checked {
                Ok(provider) => result.providers.push(provider.into()),
                Err(kind) => result.diagnostics.push(ConfigDiagnostic {
                    entry_index: index,
                    kind,
                }),
            }
        }
        if self.providers.len() > MAX_PROVIDERS {
            result.diagnostics.push(ConfigDiagnostic {
                entry_index: MAX_PROVIDERS,
                kind: "provider_capacity",
            });
        }
        if pending_legacy {
            let provider = ProviderConfig {
                id: LEGACY_ID.into(),
                name: "Legacy Depot".into(),
                endpoint: legacy.url.clone().unwrap_or_default(),
                enabled: legacy.enabled.unwrap_or(true),
                auth_mode: AuthMode::Bearer,
                bearer_token_env: Some("LABBY_DEPOT_TOKEN".into()),
            };
            let error = if counts.contains_key(LEGACY_ID) {
                Some("legacy_collision")
            } else if legacy.url.is_none() {
                Some("legacy_url_required")
            } else if provider.enabled && !legacy.token_present {
                Some("credential_required")
            } else {
                provider.validate().err()
            };
            if let Some(kind) = error {
                result.diagnostics.push(ConfigDiagnostic {
                    entry_index: MAX_PROVIDERS,
                    kind,
                });
            } else {
                result.providers.push(provider.into());
            }
        }
        result
    }
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_provider_id(&self.id) {
            return Err("invalid_id");
        }
        if !(1..=128).contains(&self.name.chars().count()) {
            return Err("invalid_name");
        }
        canonical_endpoint(&self.endpoint)?;
        match (&self.auth_mode, &self.bearer_token_env) {
            (AuthMode::Anonymous, Some(_)) => Err("unexpected_credential"),
            (AuthMode::Bearer, Some(key)) if allowed_secret_reference(key) => Ok(()),
            (AuthMode::Bearer, _) => Err("credential_reference_required"),
            _ => Ok(()),
        }
    }
}

impl From<ProviderConfig> for ProviderView {
    fn from(p: ProviderConfig) -> Self {
        Self {
            id: p.id,
            name: p.name,
            endpoint: p.endpoint,
            enabled: p.enabled,
            auth_mode: p.auth_mode,
            bearer_token_env: p.bearer_token_env,
        }
    }
}

pub fn valid_provider_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id != "all"
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

pub fn allowed_secret_reference(key: &str) -> bool {
    key.len() <= 128
        && key.starts_with("LABBY_DEPOT_")
        && key.ends_with("_TOKEN")
        && key
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

pub fn canonical_endpoint(raw: &str) -> Result<url::Url, &'static str> {
    if raw.len() > 2048 || raw.trim() != raw {
        return Err("invalid_endpoint");
    }
    let mut url = url::Url::parse(raw).map_err(|_| "invalid_endpoint")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_endpoint");
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ArtifactRefWire")]
pub struct ArtifactRef {
    pub provider_id: String,
    pub artifact_id: String,
}

#[derive(Deserialize)]
struct ArtifactRefWire {
    provider_id: String,
    artifact_id: String,
}

impl TryFrom<ArtifactRefWire> for ArtifactRef {
    type Error = &'static str;
    fn try_from(raw: ArtifactRefWire) -> Result<Self, Self::Error> {
        Self::new(&raw.provider_id, &raw.artifact_id)
    }
}

impl ArtifactRef {
    pub fn new(provider_id: &str, artifact_id: &str) -> Result<Self, &'static str> {
        if !valid_provider_id(provider_id) || !(1..=2048).contains(&artifact_id.len()) {
            return Err("invalid_artifact_identity");
        }
        Ok(Self {
            provider_id: provider_id.into(),
            artifact_id: artifact_id.into(),
        })
    }
}

pub fn safe_total(value: u64) -> Option<u64> {
    (value <= MAX_SAFE_INTEGER).then_some(value)
}
