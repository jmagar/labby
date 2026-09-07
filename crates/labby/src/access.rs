mod authorization;
mod bootstrap;
mod credential_schema;
mod credential_store;
mod credential_verifier;
mod domain;
mod error;
#[cfg(feature = "gateway")]
mod gateway_loadout;
mod health;
mod integrity;
mod loadout;
mod migrations;
#[cfg(test)]
pub(crate) mod migration_fixture {
    pub(crate) const APPLICATION_ID: i64 = super::migrations::APPLICATION_ID;
    pub(crate) const DOMAIN_SCHEMA: &str = super::migrations::DOMAIN_SCHEMA;
    pub(crate) const V1_METADATA_SCHEMA: &str = super::migrations::V1_METADATA_SCHEMA;
    pub(crate) const V1_SCHEMA_FINGERPRINT: &str = super::migrations::V1_SCHEMA_FINGERPRINT;
    pub(crate) const V1_SCHEMA_VERSION: i64 = super::migrations::V1_SCHEMA_VERSION;
}
mod read;
mod resolver;
mod runtime;
mod store;
#[cfg(test)]
mod test_support;
mod workflow;

/// Durable principal identity resolved from a live [`labby_auth::PrincipalLink`]
/// by AccessStore. The private field prevents storage services from inventing
/// identities from actor keys or presentation metadata.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AccessPrincipalId(String);

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct FileStashRecipient {
    pub(crate) principal_id: String,
    pub(crate) display_name: String,
}

impl AccessPrincipalId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Rehydrate an ID that was minted by this process for its private
    /// in-process MCP peer. Network callers must never reach this constructor.
    pub(crate) fn from_propagated(value: String) -> Option<Self> {
        (!value.is_empty() && value.len() <= 255).then_some(Self(value))
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Keeps AccessStore mutation admission leased after a fresh active-principal
/// read, so a grant commit can linearize ahead of recipient deactivation.
pub(crate) struct ActiveFileStashPrincipalLease {
    _guards: Vec<tokio::sync::OwnedRwLockReadGuard<()>>,
}

#[allow(unused_imports)]
pub(crate) use authorization::{
    AuthorizeProjectInput, LibraryAccessSnapshot, ProjectPermissionSnapshot,
};
#[allow(unused_imports)]
pub(crate) use bootstrap::{BootstrapOutcome, BootstrapOwnerInput};
pub(crate) use credential_store::{
    ActivateProofInput, ConsumeBootstrapInput, CredentialSnapshot, IssueCredentialInput,
    MutationOutcome,
};
#[allow(unused_imports)]
pub(crate) use credential_verifier::{
    AccessCredentialAdapter, LiveAuthority, LiveAuthorityError, LiveAuthorityFuture,
    LiveAuthoritySnapshot, ProtectedCredentialRequirements, StoredBinding, VerifiedProductBinding,
};
#[allow(unused_imports)]
pub(crate) use domain::{Permission, ProjectRole};
pub(crate) use error::AccessStoreError;
#[cfg(feature = "gateway")]
#[allow(unused_imports)]
pub(crate) use gateway_loadout::{GatewayLoadoutAssignmentError, assign_admitted_project_loadout};
#[cfg(feature = "gateway")]
#[allow(unused_imports)]
pub(crate) use gateway_loadout::{
    ProjectRuntimeLoadoutContext, ProjectRuntimeLoadoutError, ProjectRuntimeMcpCatalogContext,
    ProjectRuntimeMcpCatalogError, project_runtime_loadout_context,
    project_runtime_mcp_catalog_context,
};
pub(crate) use health::{AccessHealth, AccessHealthStatus, inspect_health};
#[allow(unused_imports)]
pub(crate) use loadout::{AssignProjectLoadoutInput, AssignProjectLoadoutOutcome};
#[allow(unused_imports)]
pub(crate) use read::{AccessibleProjectSnapshot, ProjectAccessSnapshot};
pub(crate) use runtime::CredentialLifecycleError;
#[allow(unused_imports)]
pub(crate) use runtime::FileStashPrincipalResolutionError;
#[allow(unused_imports)]
pub(crate) use runtime::{
    AccessBlockedReason, AccessRuntime, AccessRuntimeError, AccessRuntimeStatus, AccessSetupReason,
};
#[allow(unused_imports)]
pub(crate) use store::AccessStore;
#[allow(unused_imports)]
pub(crate) use workflow::{OwnerBootstrapError, bootstrap_owner};

#[cfg(test)]
mod facade_tests {
    #[test]
    fn bootstrap_facade_is_crate_private_callable() {
        fn accepts(_: super::BootstrapOwnerInput) {}
        fn returns(_: super::BootstrapOutcome) {}
        fn workflow_errors(_: super::OwnerBootstrapError) {}
        let _ = (accepts, returns, workflow_errors);
    }
}
