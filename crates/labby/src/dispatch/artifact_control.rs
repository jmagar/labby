//! Server-held remote Artifact authority used by the public control-plane services.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use labby_apis::artifact_control::{ArtifactControlClient, Operation};
use labby_apis::core::{ApiError, Auth, HttpClient};
use labby_auth::VerifiedIdentity;
use labby_auth::depot_delegation::{
    ASSERTION_AUDIENCE, ASSERTION_ISSUER, DelegatedAuthorityEpochs, DepotDelegationClaims,
    DepotDelegationSigner,
};
use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::Value;

use crate::config::{ArtifactPreferences, ArtifactSourceKind};
use crate::dispatch::error::ToolError;

const REMOTE_CONNECTION: ParamSpec = ParamSpec {
    name: "connection_id",
    ty: "string",
    required: false,
    description: "Configured remote Artifact authority; optional when exactly one is configured",
};
const REMOTE_CURSOR: ParamSpec = ParamSpec {
    name: "cursor",
    ty: "string",
    required: false,
    description: "Opaque remote continuation cursor",
};
const REMOTE_LIMIT: ParamSpec = ParamSpec {
    name: "limit",
    ty: "integer",
    required: false,
    description: "Bounded remote page size",
};

pub(crate) const CALLBACK_REMOTE_ACTIONS: [ActionSpec; 4] = [
    ActionSpec {
        name: "artifacts.search_remote",
        description: "Search the configured remote Artifact catalog",
        destructive: false,
        requires_admin: false,
        returns: "RemoteArtifactSearch",
        params: &[
            REMOTE_CONNECTION,
            ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Case-insensitive remote catalog query",
            },
            REMOTE_LIMIT,
        ],
    },
    ActionSpec {
        name: "artifacts.list_remote",
        description: "List the combined hosted and projected remote Artifact catalog",
        destructive: false,
        requires_admin: false,
        returns: "RemoteArtifactPage",
        params: &[REMOTE_CONNECTION, REMOTE_CURSOR, REMOTE_LIMIT],
    },
    ActionSpec {
        name: "artifacts.get_remote",
        description: "Get one remote Artifact by stable identifier",
        destructive: false,
        requires_admin: false,
        returns: "RemoteArtifact",
        params: &[
            REMOTE_CONNECTION,
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Stable item identifier",
            },
        ],
    },
    ActionSpec {
        name: "artifacts.list_candidates",
        description: "List remote discovery candidates awaiting intake",
        destructive: false,
        requires_admin: true,
        returns: "ArtifactCandidatePage",
        params: &[REMOTE_CONNECTION, REMOTE_CURSOR, REMOTE_LIMIT],
    },
];

#[derive(Debug, Clone)]
pub(crate) struct AuthorityContext {
    pub actor_id: String,
    pub organization_id: String,
    pub team_id: Option<String>,
    pub project_id: String,
    pub platform_administrator: bool,
    /// Exact permission revalidated for this request. Delegation must never
    /// mint a capability outside this local authorization ceiling.
    pub permission: crate::access::Permission,
    pub epochs: DelegatedAuthorityEpochs,
}

pub(crate) async fn authorize_authority_context(
    runtime: &crate::access::AccessRuntime,
    identity: VerifiedIdentity,
    project_id: &str,
    selected_team_id: Option<&str>,
    permission: crate::access::Permission,
) -> Result<AuthorityContext, ToolError> {
    let store = runtime.store().await.map_err(|_| ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Artifact authorization is unavailable".to_owned(),
    })?;
    store
        .authorize_skill_library(identity.clone(), project_id.to_owned(), permission)
        .await
        .map_err(|_| ToolError::Forbidden {
            message: "Remote Artifact operation is not authorized for this project".to_owned(),
            required_scopes: vec!["lab:read".to_owned()],
        })?;
    let snapshot = store
        .depot_delegation_authority(
            identity,
            project_id.to_owned(),
            selected_team_id.map(str::to_owned),
        )
        .await
        .map_err(|_| ToolError::Forbidden {
            message: "Remote Artifact operation requires one explicit authorized team".to_owned(),
            required_scopes: vec!["lab:read".to_owned()],
        })?;
    Ok(AuthorityContext {
        actor_id: snapshot.principal_id,
        organization_id: snapshot.organization_id,
        team_id: snapshot.team_id,
        project_id: snapshot.project_id,
        platform_administrator: snapshot.platform_administrator,
        permission,
        epochs: DelegatedAuthorityEpochs {
            authority_schema: snapshot.authority_schema,
            organization_policy: snapshot.organization_policy,
            team_membership: snapshot.team_membership,
            team_policy: snapshot.team_policy,
            project_membership: snapshot.project_membership,
            project_policy: Some(snapshot.project_policy),
            global_revision: snapshot.global_revision,
        },
    })
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ArtifactControlPlane {
    clients: BTreeMap<String, AuthorityConnection>,
    delegation: Option<Arc<DelegationConfiguration>>,
}

struct DelegationConfiguration {
    signer: DepotDelegationSigner,
    deployment_id: String,
}

impl std::fmt::Debug for DelegationConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationConfiguration")
            .field("deployment_id", &self.deployment_id)
            .field("signer", &self.signer)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct AuthorityConnection {
    control_plane_url: String,
    pinned_addresses: Vec<IpAddr>,
    bearer_token_env: Option<String>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl ArtifactControlPlane {
    #[cfg(test)]
    pub(crate) fn from_config(config: &ArtifactPreferences) -> Result<Self, ToolError> {
        Self::from_configs(config, &crate::config::depot::DepotPreferences::default())
    }

    pub(crate) fn from_configs(
        config: &ArtifactPreferences,
        depot: &crate::config::depot::DepotPreferences,
    ) -> Result<Self, ToolError> {
        if config.sources.iter().any(|source| {
            source.kind == ArtifactSourceKind::Repository && source.control_plane_url.is_some()
        }) {
            return Err(ToolError::InvalidParam {
                message: "control_plane_url is supported only for Depot sources".to_owned(),
                param: "control_plane_url".to_owned(),
            });
        }
        let mut clients = BTreeMap::new();
        for source in config.sources.iter().filter(|source| {
            source.kind == ArtifactSourceKind::Depot && source.control_plane_url.is_some()
        }) {
            if clients.contains_key(&source.id) {
                return Err(ToolError::Conflict {
                    message: "Duplicate Artifact authority connection".to_owned(),
                    existing_id: source.id.clone(),
                });
            }
            let control_plane_url = source
                .control_plane_url
                .as_deref()
                .expect("filtered to configured control-plane URLs");
            let parsed = labby_primitives::ssrf::parse_validated_https_url(control_plane_url)
                .map_err(|_| ToolError::InvalidParam {
                    message: "Artifact control-plane URL must be a public HTTPS origin".to_owned(),
                    param: "control_plane_url".to_owned(),
                })?;
            if parsed.path() != "/" {
                return Err(ToolError::InvalidParam {
                    message: "Artifact control-plane URL must not include a path".to_owned(),
                    param: "control_plane_url".to_owned(),
                });
            }
            for address in &source.pinned_addresses {
                labby_primitives::ssrf::check_ip_not_private(*address, "Artifact authority")
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact authority pin must be a public address".to_owned(),
                        param: "pinned_addresses".to_owned(),
                    })?;
            }
            clients.insert(
                source.id.clone(),
                AuthorityConnection {
                    control_plane_url: control_plane_url.to_owned(),
                    pinned_addresses: source.pinned_addresses.clone(),
                    bearer_token_env: source.bearer_token_env.clone(),
                    permits: Arc::new(tokio::sync::Semaphore::new(16)),
                },
            );
        }
        let delegation = delegation_configuration(depot)?;
        Ok(Self {
            clients,
            delegation,
        })
    }

    pub(crate) async fn execute(
        &self,
        connection_id: Option<&str>,
        operation: Operation,
        params: &Value,
        context: Option<&AuthorityContext>,
    ) -> Result<Value, ToolError> {
        let connection = self.connection(connection_id)?;
        let _permit = tokio::time::timeout(Duration::from_secs(2), connection.permits.acquire())
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "queue_saturated".to_owned(),
                message: "Artifact authority request queue is saturated".to_owned(),
            })?
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "source_unavailable".to_owned(),
                message: "Artifact authority connection is unavailable".to_owned(),
            })?;
        let client = connection.client(context)?;
        let headers = self.delegation_headers(operation, params, context)?;
        let result = client
            .execute_with_headers(operation, params, headers)
            .await
            .map_err(map_api_error)?;
        Ok(redact_provider_metadata(result))
    }

    pub(crate) async fn upload(
        &self,
        connection_id: Option<&str>,
        upload_id: &str,
        body: reqwest::Body,
        content_length: Option<u64>,
        content_type: &str,
        content_digest: &str,
        context: &AuthorityContext,
    ) -> Result<Value, ToolError> {
        let connection = self.connection(connection_id)?;
        let _permit = tokio::time::timeout(Duration::from_secs(2), connection.permits.acquire())
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "queue_saturated".to_owned(),
                message: "Artifact authority request queue is saturated".to_owned(),
            })?
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "source_unavailable".to_owned(),
                message: "Artifact authority connection is unavailable".to_owned(),
            })?;
        let client = connection.client(Some(context))?;
        let headers =
            self.upload_delegation_headers(upload_id, content_digest, content_length, context)?;
        let result = client
            .upload_with_headers(upload_id, body, content_length, content_type, headers)
            .await
            .map_err(map_api_error)?;
        Ok(redact_provider_metadata(result))
    }

    fn connection(&self, connection_id: Option<&str>) -> Result<&AuthorityConnection, ToolError> {
        match connection_id {
            Some(id) => self.clients.get(id),
            None if self.clients.len() == 1 => self.clients.values().next(),
            None => None,
        }
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: if connection_id.is_none() && self.clients.len() > 1 {
                "Multiple Artifact authorities are configured; connection_id is required".to_owned()
            } else {
                "Requested Artifact authority is not configured".to_owned()
            },
        })
    }

    pub(crate) fn connections(&self) -> Value {
        let connections = self
            .clients
            .keys()
            .map(|id| serde_json::json!({ "id": id }))
            .collect::<Vec<_>>();
        serde_json::json!({
            "connections": connections,
            "default_connection_id": (self.clients.len() == 1)
                .then(|| self.clients.keys().next().cloned())
                .flatten(),
        })
    }

    fn delegation_headers(
        &self,
        operation: Operation,
        params: &Value,
        context: Option<&AuthorityContext>,
    ) -> Result<reqwest::header::HeaderMap, ToolError> {
        let Some(delegation) = self.delegation.as_ref() else {
            return Ok(reqwest::header::HeaderMap::new());
        };
        let context = context.ok_or_else(delegation_unavailable)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| delegation_unavailable())?
            .as_secs();
        let intent_id = params
            .get("idempotencyKey")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let resource = format!("/api/operations/{}", operation.provider_name());
        let claims = DepotDelegationClaims {
            iss: ASSERTION_ISSUER.into(),
            sub: context.actor_id.clone(),
            aud: ASSERTION_AUDIENCE.into(),
            iat: now,
            nbf: now,
            exp: now + 30,
            jti: uuid::Uuid::new_v4().simple().to_string(),
            deployment_id: delegation.deployment_id.clone(),
            account_id: context.organization_id.clone(),
            organization_id: context.organization_id.clone(),
            team_id: context.team_id.clone(),
            project_id: Some(context.project_id.clone()),
            principal_id: context.actor_id.clone(),
            method: "POST".into(),
            resource,
            operation: operation.provider_name().into(),
            intent_id: intent_id.clone(),
            content_digest: None,
            content_length: None,
            scopes: operation_scopes(operation, context.permission)?
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
            capabilities: operation_capabilities(operation, context)?,
            epochs: context.epochs.clone(),
            delegation_chain: vec!["labby".into()],
        };
        let assertion = delegation
            .signer
            .issue(claims)
            .map_err(|_| delegation_unavailable())?;
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in [
            ("x-labby-delegation", assertion.as_str()),
            ("x-labby-organization-id", context.organization_id.as_str()),
            ("x-labby-project-id", context.project_id.as_str()),
            ("idempotency-key", intent_id.as_str()),
        ] {
            headers.insert(name, value.parse().map_err(|_| delegation_unavailable())?);
        }
        if let Some(team_id) = &context.team_id {
            headers.insert(
                "x-labby-team-id",
                team_id.parse().map_err(|_| delegation_unavailable())?,
            );
        }
        Ok(headers)
    }

    fn upload_delegation_headers(
        &self,
        upload_id: &str,
        content_digest: &str,
        content_length: Option<u64>,
        context: &AuthorityContext,
    ) -> Result<reqwest::header::HeaderMap, ToolError> {
        let Some(delegation) = self.delegation.as_ref() else {
            return Ok(reqwest::header::HeaderMap::new());
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| delegation_unavailable())?
            .as_secs();
        let intent_id = uuid::Uuid::new_v4().simple().to_string();
        let resource = format!("/uploads/{}", HttpClient::encode_path_segment(upload_id));
        let claims = DepotDelegationClaims {
            iss: ASSERTION_ISSUER.into(),
            sub: context.actor_id.clone(),
            aud: ASSERTION_AUDIENCE.into(),
            iat: now,
            nbf: now,
            exp: now + 30,
            jti: uuid::Uuid::new_v4().simple().to_string(),
            deployment_id: delegation.deployment_id.clone(),
            account_id: context.organization_id.clone(),
            organization_id: context.organization_id.clone(),
            team_id: context.team_id.clone(),
            project_id: Some(context.project_id.clone()),
            principal_id: context.actor_id.clone(),
            method: "PUT".into(),
            resource,
            operation: "depot.uploads.put".into(),
            intent_id: intent_id.clone(),
            content_digest: Some(content_digest.to_owned()),
            content_length,
            scopes: operation_scopes(Operation::UploadsCreate, context.permission)?
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
            capabilities: operation_capabilities(Operation::UploadsCreate, context)?,
            epochs: context.epochs.clone(),
            delegation_chain: vec!["labby".into()],
        };
        let assertion = delegation
            .signer
            .issue(claims)
            .map_err(|_| delegation_unavailable())?;
        delegation_header_map(context, &assertion, &intent_id)
    }
}

fn delegation_header_map(
    context: &AuthorityContext,
    assertion: &str,
    intent_id: &str,
) -> Result<reqwest::header::HeaderMap, ToolError> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in [
        ("x-labby-delegation", assertion),
        ("x-labby-organization-id", context.organization_id.as_str()),
        ("x-labby-project-id", context.project_id.as_str()),
        ("idempotency-key", intent_id),
    ] {
        headers.insert(name, value.parse().map_err(|_| delegation_unavailable())?);
    }
    if let Some(team_id) = &context.team_id {
        headers.insert(
            "x-labby-team-id",
            team_id.parse().map_err(|_| delegation_unavailable())?,
        );
    }
    Ok(headers)
}

fn operation_capabilities(
    operation: Operation,
    context: &AuthorityContext,
) -> Result<Vec<String>, ToolError> {
    let (capability, required_permission) = operation_authority(operation);
    if context.permission != required_permission {
        return Err(ToolError::Forbidden {
            message: "Remote Artifact operation exceeds its authorized permission".to_owned(),
            required_scopes: Vec::new(),
        });
    }
    if context.platform_administrator {
        return Ok(vec!["platform.manage".into()]);
    }
    Ok(vec![capability.into()])
}

fn operation_authority(operation: Operation) -> (&'static str, crate::access::Permission) {
    match operation {
        Operation::ArtifactsList
        | Operation::ArtifactsSearch
        | Operation::SearchSkillsSh
        | Operation::SearchArd
        | Operation::SearchMarketplace
        | Operation::McpRegistryList
        | Operation::AcpRegistryList
        | Operation::AuthorityStatus
        | Operation::SourcesList
        | Operation::BundlesList => ("scope.read", crate::access::Permission::AssetDiscover),
        Operation::CandidatesList
        | Operation::JobsList
        | Operation::JobsGet
        | Operation::UploadsGet => ("scope.read", crate::access::Permission::ProjectManage),
        Operation::ArtifactsGet | Operation::BundlesGet => {
            ("scope.use", crate::access::Permission::AssetUse)
        }
        Operation::CandidatesIntake
        | Operation::ArtifactsFork
        | Operation::JobsStart
        | Operation::UploadsCreate
        | Operation::BundlesCreate
        | Operation::BundlesAddArtifact => {
            ("scope.create", crate::access::Permission::ProjectManage)
        }
        Operation::JobsCancel
        | Operation::JobsRetry
        | Operation::BundlesPublish
        | Operation::BundlesRemoveArtifact => {
            ("scope.operate", crate::access::Permission::ProjectManage)
        }
        _ => ("scope.manage", crate::access::Permission::ProjectManage),
    }
}

pub(crate) fn operation_permission(operation: Operation) -> crate::access::Permission {
    operation_authority(operation).1
}

fn operation_scopes(
    operation: Operation,
    permission: crate::access::Permission,
) -> Result<&'static [&'static str], ToolError> {
    // Validate the permission/capability pair before emitting broader transport
    // scopes. Depot treats both fields as authority, so neither may exceed the
    // exact Labby decision.
    let (_, required_permission) = operation_authority(operation);
    if permission != required_permission {
        return Err(ToolError::Forbidden {
            message: "Remote Artifact operation exceeds its authorized permission".to_owned(),
            required_scopes: Vec::new(),
        });
    }
    Ok(match permission {
        crate::access::Permission::AssetDiscover | crate::access::Permission::AssetUse => {
            &["skills:read"]
        }
        crate::access::Permission::ProjectManage => &["skills:read", "skills:write"],
        crate::access::Permission::ProjectRead => &["skills:read"],
    })
}

fn delegation_configuration(
    depot: &crate::config::depot::DepotPreferences,
) -> Result<Option<Arc<DelegationConfiguration>>, ToolError> {
    use base64::Engine as _;
    if depot.control_mode != crate::config::depot::DepotControlMode::LabbyManaged {
        return Ok(None);
    }
    let configured = [
        depot.authority_installation_id.as_ref(),
        depot.authority_key_id.as_ref(),
        depot.authority_signing_key_env.as_ref(),
    ];
    if configured.iter().all(|value| value.is_none()) {
        return Err(delegation_unavailable());
    }
    let [deployment_id, key_id, key_env] = configured;
    let Some(deployment_id) = deployment_id else {
        return Err(delegation_unavailable());
    };
    let Some(key_id) = key_id else {
        return Err(delegation_unavailable());
    };
    let Some(key_env) = key_env else {
        return Err(delegation_unavailable());
    };
    let encoded = std::env::var(key_env).map_err(|_| delegation_unavailable())?;
    let seed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .map_err(|_| delegation_unavailable())?;
    let seed: [u8; 32] = seed.try_into().map_err(|_| delegation_unavailable())?;
    delegation_configuration_from_seed(deployment_id, key_id, seed).map(Some)
}

fn delegation_configuration_from_seed(
    deployment_id: &str,
    key_id: &str,
    seed: [u8; 32],
) -> Result<Arc<DelegationConfiguration>, ToolError> {
    const PKCS8_PREFIX: &[u8] = &[
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    let mut der = PKCS8_PREFIX.to_vec();
    der.extend_from_slice(&seed);
    let signer = DepotDelegationSigner::new(key_id.to_owned(), [(key_id.to_owned(), der)])
        .map_err(|_| delegation_unavailable())?;
    Ok(Arc::new(DelegationConfiguration {
        signer,
        deployment_id: deployment_id.to_owned(),
    }))
}

fn delegation_unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Managed Depot delegation authority is unavailable".to_owned(),
    }
}

impl AuthorityConnection {
    fn client(
        &self,
        context: Option<&AuthorityContext>,
    ) -> Result<ArtifactControlClient, ToolError> {
        let token = self
            .bearer_token_env
            .as_ref()
            .map(|name| std::env::var(name))
            .transpose()
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "source_unavailable".to_owned(),
                message: "Artifact authority credential is unavailable".to_owned(),
            })?;
        let auth = token.map_or(Auth::None, |token| Auth::Bearer { token });
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(context) = context {
            headers.insert(
                "x-labby-actor-id",
                context
                    .actor_id
                    .parse()
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact authority actor identity is invalid".to_owned(),
                        param: "actor_id".to_owned(),
                    })?,
            );
            headers.insert(
                "x-labby-project-id",
                context
                    .project_id
                    .parse()
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact authority project identity is invalid".to_owned(),
                        param: "project_id".to_owned(),
                    })?,
            );
        }
        HttpClient::with_pinned_addresses_and_headers(
            &self.control_plane_url,
            auth,
            self.pinned_addresses.iter().copied(),
            headers,
        )
        .map(ArtifactControlClient::new)
        .map_err(map_api_error)
    }
}

fn map_api_error(error: ApiError) -> ToolError {
    match error {
        ApiError::Auth => ToolError::Forbidden {
            message: "Artifact authority rejected its server credential".to_owned(),
            required_scopes: Vec::new(),
        },
        ApiError::NotFound => ToolError::Sdk {
            sdk_kind: "not_found".to_owned(),
            message: "Remote Artifact control-plane item was not found".to_owned(),
        },
        ApiError::RateLimited { .. } => ToolError::Sdk {
            sdk_kind: "rate_limited".to_owned(),
            message: "Artifact authority is rate limited; retry later".to_owned(),
        },
        ApiError::Validation { field, .. } => ToolError::InvalidParam {
            message: "Artifact authority rejected a parameter".to_owned(),
            param: field,
        },
        ApiError::Network(_) => ToolError::Sdk {
            sdk_kind: "service_unavailable".to_owned(),
            message: "Artifact authority is unreachable".to_owned(),
        },
        ApiError::Server { status: 409, .. } => ToolError::Conflict {
            message: "Artifact authority state conflicts with this request".to_owned(),
            existing_id: "remote_artifact_state".to_owned(),
        },
        ApiError::Server { .. } => ToolError::Sdk {
            sdk_kind: "service_unavailable".to_owned(),
            message: "Artifact authority operation failed".to_owned(),
        },
        ApiError::Decode(_) | ApiError::Internal(_) => ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "Artifact authority returned an invalid response".to_owned(),
        },
    }
}

fn redact_provider_metadata(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .filter_map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase();
                    let compact = normalized
                        .chars()
                        .filter(|character| character.is_ascii_alphanumeric())
                        .collect::<String>();
                    let safe_opaque_token =
                        matches!(compact.as_str(), "pagetoken" | "nextpagetoken");
                    let sensitive = normalized.contains("authorization")
                        || normalized.contains("credential")
                        || normalized.contains("secret")
                        || normalized.contains("operator")
                        || normalized.contains("internal")
                        || normalized.contains("password")
                        || normalized.contains("apikey")
                        || normalized.contains("api_key")
                        || normalized.contains("privatekey")
                        || normalized.contains("private_key")
                        || normalized.contains("cookie")
                        || (normalized.contains("token") && !safe_opaque_token)
                        || matches!(
                            compact.as_str(),
                            "token" | "accesstoken" | "bearertoken" | "refreshtoken" | "idtoken"
                        )
                        || normalized == "raw_error"
                        || normalized == "stacktrace";
                    (!sensitive).then(|| (key, redact_provider_metadata(value)))
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_provider_metadata).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use base64::Engine as _;
    use serde_json::json;

    use super::{
        ArtifactControlPlane, AuthorityContext, Operation, operation_capabilities,
        redact_provider_metadata,
    };
    use crate::config::{ArtifactPreferences, ArtifactSourceConfig, ArtifactSourceKind};

    #[test]
    fn strips_security_and_operator_fields_but_preserves_product_metadata() {
        let projected = redact_provider_metadata(json!({
            "artifact":{"id":"a", "description":"demo", "licenseEvidence":["MIT"]},
            "credentialRef":"git-main",
            "operatorNotes":"private",
            "nested":{"accessToken":"nope", "pageToken":"continue-opaque", "provenance":{"repository":"repo"}}
        }));
        assert_eq!(projected["artifact"]["id"], "a");
        assert_eq!(projected["artifact"]["licenseEvidence"][0], "MIT");
        assert_eq!(projected["nested"]["provenance"]["repository"], "repo");
        assert_eq!(projected["nested"]["pageToken"], "continue-opaque");
        assert!(projected.get("credentialRef").is_none());
        assert!(projected["nested"].get("accessToken").is_none());
    }

    #[test]
    fn strips_conventional_secret_spellings_and_preserves_page_tokens() {
        let projected = redact_provider_metadata(json!({
            "password": "nope",
            "apiKey": "nope",
            "private_key": "nope",
            "sessionCookie": "nope",
            "githubTokenValue": "nope",
            "pageToken": "safe-page",
            "next_page_token": "safe-next"
        }));
        for key in [
            "password",
            "apiKey",
            "private_key",
            "sessionCookie",
            "githubTokenValue",
        ] {
            assert!(projected.get(key).is_none(), "{key} must be redacted");
        }
        assert_eq!(projected["pageToken"], "safe-page");
        assert_eq!(projected["next_page_token"], "safe-next");
    }

    #[test]
    fn control_plane_origin_and_pins_fail_closed() {
        let source = |url: &str, pin: IpAddr| ArtifactSourceConfig {
            id: "primary".to_owned(),
            kind: ArtifactSourceKind::Depot,
            endpoint: "https://depot.example/v1/exact".to_owned(),
            control_plane_url: Some(url.to_owned()),
            pinned_addresses: vec![pin],
            bearer_token_env: None,
        };
        let with = |source| ArtifactPreferences {
            sources: vec![source],
        };

        assert!(
            ArtifactControlPlane::from_config(&with(source(
                "https://depot.example/api",
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            )))
            .is_err()
        );
        assert!(
            ArtifactControlPlane::from_config(&with(source(
                "https://depot.example",
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            )))
            .is_err()
        );
        assert!(
            ArtifactControlPlane::from_config(&with(source(
                "https://depot.example",
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            )))
            .is_ok()
        );

        let repository = ArtifactSourceConfig {
            id: "repo".to_owned(),
            kind: ArtifactSourceKind::Repository,
            endpoint: "https://repository.example/v1/exact".to_owned(),
            control_plane_url: Some("https://depot.example".to_owned()),
            pinned_addresses: Vec::new(),
            bearer_token_env: None,
        };
        assert!(ArtifactControlPlane::from_config(&with(repository)).is_err());
    }

    #[test]
    fn missing_remote_credential_does_not_prevent_local_startup() {
        let config = ArtifactPreferences {
            sources: vec![ArtifactSourceConfig {
                id: "remote".to_owned(),
                kind: ArtifactSourceKind::Depot,
                endpoint: "https://depot.example/v1/exact".to_owned(),
                control_plane_url: Some("https://depot.example".to_owned()),
                pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
                bearer_token_env: Some("LABBY_TEST_DEFINITELY_MISSING_REMOTE_TOKEN".to_owned()),
            }],
        };
        let controls = ArtifactControlPlane::from_config(&config).unwrap();
        assert_eq!(controls.connections()["connections"][0]["id"], "remote");
        let error = controls.clients["remote"].client(None).unwrap_err();
        assert_eq!(error.kind(), "source_unavailable");
        assert!(!error.to_string().contains("LABBY_TEST"));
    }

    #[test]
    fn connection_discovery_exposes_only_safe_ids() {
        let config = ArtifactPreferences {
            sources: vec![ArtifactSourceConfig {
                id: "primary".to_owned(),
                kind: ArtifactSourceKind::Depot,
                endpoint: "https://depot.example/v1/exact".to_owned(),
                control_plane_url: Some("https://depot.example".to_owned()),
                pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
                bearer_token_env: Some("PRIVATE_REMOTE_TOKEN".to_owned()),
            }],
        };
        let value = ArtifactControlPlane::from_config(&config)
            .unwrap()
            .connections();
        assert_eq!(value["default_connection_id"], "primary");
        let encoded = value.to_string();
        assert!(!encoded.contains("depot.example"));
        assert!(!encoded.contains("PRIVATE_REMOTE_TOKEN"));
    }

    #[test]
    fn managed_delegation_is_fresh_and_exactly_request_bound() {
        let controls = ArtifactControlPlane {
            clients: Default::default(),
            delegation: Some(
                super::delegation_configuration_from_seed("deployment-1", "current", [7_u8; 32])
                    .unwrap(),
            ),
        };
        let context = AuthorityContext {
            actor_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            team_id: Some("team-1".into()),
            project_id: "project-1".into(),
            platform_administrator: false,
            permission: crate::access::Permission::ProjectManage,
            epochs: labby_auth::depot_delegation::DelegatedAuthorityEpochs {
                authority_schema: 7,
                organization_policy: 8,
                team_membership: Some(9),
                team_policy: Some(10),
                project_membership: Some(11),
                project_policy: Some(12),
                global_revision: 13,
            },
        };
        let headers = controls
            .delegation_headers(
                Operation::JobsStart,
                &json!({"idempotencyKey":"intent-1"}),
                Some(&context),
            )
            .unwrap();
        assert_eq!(headers["idempotency-key"], "intent-1");
        assert_eq!(headers["x-labby-team-id"], "team-1");
        assert_eq!(headers["x-labby-organization-id"], "organization-1");
        assert_eq!(headers["x-labby-project-id"], "project-1");
        let token = headers["x-labby-delegation"].to_str().unwrap();
        let payload = token.split('.').nth(1).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["method"], "POST");
        assert_eq!(claims["resource"], "/api/operations/depot.ingest.start");
        assert_eq!(claims["operation"], "depot.ingest.start");
        assert_eq!(claims["intent_id"], "intent-1");
        assert_eq!(claims["capabilities"], json!(["scope.create"]));
        assert!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap() <= 60);

        let upload_headers = controls
            .upload_delegation_headers(
                "upload-1",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some(12),
                &context,
            )
            .unwrap();
        let upload_token = upload_headers["x-labby-delegation"].to_str().unwrap();
        let upload_payload = upload_token.split('.').nth(1).unwrap();
        let upload_claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(upload_payload)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            upload_claims["content_digest"],
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(upload_claims["content_length"], 12);
    }

    #[test]
    fn managed_delegation_configuration_fails_closed_when_incomplete() {
        let depot = crate::config::depot::DepotPreferences {
            control_mode: crate::config::depot::DepotControlMode::LabbyManaged,
            authority_installation_id: Some("deployment-1".into()),
            ..Default::default()
        };
        assert!(
            ArtifactControlPlane::from_configs(&ArtifactPreferences::default(), &depot).is_err()
        );
    }

    #[test]
    fn delegated_capability_cannot_exceed_exact_local_permission() {
        let context = AuthorityContext {
            actor_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            team_id: Some("team-1".into()),
            project_id: "project-1".into(),
            platform_administrator: false,
            permission: crate::access::Permission::AssetDiscover,
            epochs: labby_auth::depot_delegation::DelegatedAuthorityEpochs {
                authority_schema: 1,
                organization_policy: 1,
                team_membership: Some(1),
                team_policy: Some(1),
                project_membership: Some(1),
                project_policy: Some(1),
                global_revision: 1,
            },
        };
        assert!(operation_capabilities(Operation::ArtifactsList, &context).is_ok());
        assert!(operation_capabilities(Operation::JobsStart, &context).is_err());
        assert!(operation_capabilities(Operation::SourcesConfigure, &context).is_err());
    }

    /// Cross-repository system driver. Depot's ExUnit orchestrator supplies a
    /// real production Router endpoint and credentials, then invokes this exact
    /// ignored test. No HTTP contract mock is used here.
    #[tokio::test]
    #[ignore = "run by Depot's managed Labby HTTP orchestrator"]
    async fn real_managed_depot_http_driver() {
        use crate::access::{AssignTeamProjectInput, BootstrapOwnerInput, Permission, ProjectRole};
        use labby_auth::{Authenticator, VerifiedIdentity};

        drop(rustls::crypto::ring::default_provider().install_default());

        let endpoint = std::env::var("LABBY_DEPOT_SYSTEM_ENDPOINT")
            .expect("LABBY_DEPOT_SYSTEM_ENDPOINT is required");
        let bearer_env = "LABBY_DEPOT_SYSTEM_BEARER";
        std::env::var(bearer_env).expect("LABBY_DEPOT_SYSTEM_BEARER is required");
        let seed = [41_u8; 32];
        let directory = tempfile::Builder::new()
            .prefix("labby-depot-system-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = crate::access::AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "managed-system-owner",
        )
        .unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(owner.clone(), "System Org", "System Project").unwrap(),
            )
            .await
            .unwrap();
        store
            .assign_team_project(
                AssignTeamProjectInput::new(
                    owner.clone(),
                    "bootstrap-initial-team",
                    "bootstrap-default",
                    ProjectRole::Owner,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let sender = crate::dispatch::depot::authority_projection::AuthorityProjectionSender::new(
            url::Url::parse(&endpoint).unwrap(),
            std::env::var(bearer_env).unwrap(),
            "system-installation",
            "current",
            seed,
            store.clone(),
        )
        .unwrap();
        sender
            .send_current_snapshot(
                "bootstrap-local",
                i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let controls = ArtifactControlPlane {
            clients: [(
                "system".to_owned(),
                super::AuthorityConnection {
                    control_plane_url: endpoint,
                    pinned_addresses: vec!["127.0.0.1".parse().unwrap()],
                    bearer_token_env: Some(bearer_env.into()),
                    permits: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
                },
            )]
            .into_iter()
            .collect(),
            delegation: Some(
                super::delegation_configuration_from_seed("system-installation", "current", seed)
                    .unwrap(),
            ),
        };
        let runtime =
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await;
        let context = super::authorize_authority_context(
            &runtime,
            owner,
            "bootstrap-default",
            None,
            Permission::AssetDiscover,
        )
        .await
        .unwrap();
        let result = controls
            .execute(
                Some("system"),
                Operation::ArtifactsList,
                &json!({"limit":1}),
                Some(&context),
            )
            .await
            .unwrap();
        assert!(result.get("artifacts").is_some());
    }
}
