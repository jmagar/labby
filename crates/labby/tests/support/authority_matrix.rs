use std::collections::BTreeSet;

use super::action_matrix::CatalogAction;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OwnerKind {
    Installation,
    Team,
    Project,
    Personal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResourceFamily {
    Platform,
    Library,
    Project,
    Gateway,
    Stash,
    Agent,
    Task,
    DevContainer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationClass {
    Discover,
    Read,
    Operate,
    Administer,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityClassification {
    pub(crate) resource: ResourceFamily,
    pub(crate) operation: OperationClass,
    pub(crate) owners: &'static [OwnerKind],
    pub(crate) delegated: bool,
    pub(crate) final_boundary_reauthorization: bool,
}

const INSTALLATION: &[OwnerKind] = &[OwnerKind::Installation];
const USER_OWNED: &[OwnerKind] = &[OwnerKind::Team, OwnerKind::Project, OwnerKind::Personal];
const GATEWAY_OWNED: &[OwnerKind] = &[
    OwnerKind::Installation,
    OwnerKind::Team,
    OwnerKind::Project,
    OwnerKind::Personal,
];

/// Classifies the currently registered Labby services by the authority they must
/// eventually enforce. The action intent fixture is an exact inventory of action
/// names, so this service-level policy cannot hide an unregistered action.
pub(crate) fn classify_labby(action: &CatalogAction) -> Option<AuthorityClassification> {
    let (resource, owners, delegated) = match action.service.as_str() {
        "access" if action.action.starts_with("access.platform_admin.") => {
            (ResourceFamily::Platform, INSTALLATION, false)
        }
        "access" => (ResourceFamily::Project, USER_OWNED, false),
        "agents" => (ResourceFamily::Agent, USER_OWNED, false),
        "artifacts" | "bundles" | "sources" | "uploads" => {
            (ResourceFamily::Library, USER_OWNED, true)
        }
        "gateway" => (ResourceFamily::Gateway, GATEWAY_OWNED, true),
        "browser" | "snippets" => (ResourceFamily::Gateway, USER_OWNED, true),
        "dev_containers" => (ResourceFamily::DevContainer, USER_OWNED, false),
        "jobs" => (ResourceFamily::Task, USER_OWNED, true),
        "stash" => (ResourceFamily::Stash, USER_OWNED, false),
        "tasks" => (ResourceFamily::Task, USER_OWNED, false),
        "doctor" | "fs" | "lab_admin" | "server_logs" | "setup" => {
            (ResourceFamily::Platform, INSTALLATION, false)
        }
        _ => return None,
    };
    let operation = if action.builtin {
        OperationClass::Discover
    } else if action.destructive {
        OperationClass::Delete
    } else if action.requires_admin {
        OperationClass::Administer
    } else {
        OperationClass::Read
    };
    Some(AuthorityClassification {
        resource,
        operation,
        owners,
        delegated,
        final_boundary_reauthorization: !action.builtin,
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepotOperation {
    pub(crate) name: &'static str,
    pub(crate) resource: ResourceFamily,
    pub(crate) operation: OperationClass,
    pub(crate) owners: &'static [OwnerKind],
    pub(crate) delegated: bool,
}

macro_rules! depot {
    ($name:literal, $resource:ident, $operation:ident, $owners:expr, $delegated:expr) => {
        DepotOperation {
            name: $name,
            resource: ResourceFamily::$resource,
            operation: OperationClass::$operation,
            owners: $owners,
            delegated: $delegated,
        }
    };
}

/// Snapshot of Depot's cross-product operation registry. This is intentionally
/// explicit: adding a Depot operation requires choosing its resource family,
/// operation class, owner kinds, and delegated-call posture here as well as in
/// Depot's own registry completeness test.
pub(crate) const DEPOT_OPERATIONS: &[DepotOperation] = &[
    depot!("depot.acp_registry.list", Library, Read, USER_OWNED, true),
    depot!("depot.artifacts.exact", Library, Read, USER_OWNED, true),
    depot!("depot.artifacts.follow", Library, Operate, USER_OWNED, true),
    depot!("depot.artifacts.fork", Library, Operate, USER_OWNED, true),
    depot!("depot.artifacts.get", Library, Read, USER_OWNED, true),
    depot!(
        "depot.artifacts.intake_candidate",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.artifacts.list", Library, Read, USER_OWNED, true),
    depot!(
        "depot.artifacts.list_candidates",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.artifacts.set_license",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.artifacts.set_publication",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.add_skill",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.create",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.bundles.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.bundles.get", Library, Read, USER_OWNED, true),
    depot!("depot.bundles.list", Library, Read, USER_OWNED, true),
    depot!(
        "depot.bundles.publish",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.remove_skill",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.set_visibility",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.ingest.cancel", Task, Operate, USER_OWNED, true),
    depot!("depot.ingest.get", Task, Read, USER_OWNED, true),
    depot!("depot.ingest.list", Task, Read, USER_OWNED, true),
    depot!("depot.ingest.retry", Task, Operate, USER_OWNED, true),
    depot!("depot.ingest.start", Task, Operate, USER_OWNED, true),
    depot!(
        "depot.maintenance.cas_audit",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.audit",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.copy",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.cutover",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.end_retention",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.plan",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.rollback",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.status",
        Platform,
        Read,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.verify",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.gc",
        Platform,
        Delete,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.sidecars",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.upstream",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!("depot.mcp_registry.list", Library, Read, USER_OWNED, true),
    depot!("depot.skills.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.skills.get", Library, Read, USER_OWNED, true),
    depot!(
        "depot.skills.ingest_acp_registry",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_ard_catalog",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_marketplace",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_mcp",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_mcp_registry",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_repo",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_skills_sh",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_well_known",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!("depot.skills.list", Library, Read, USER_OWNED, true),
    depot!("depot.skills.load", Library, Read, USER_OWNED, true),
    depot!("depot.skills.read", Library, Read, USER_OWNED, true),
    depot!("depot.skills.search", Library, Read, USER_OWNED, true),
    depot!("depot.skills.search_ard", Library, Read, USER_OWNED, true),
    depot!(
        "depot.skills.search_marketplace",
        Library,
        Read,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.search_skills_sh",
        Library,
        Read,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.sources.configure",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.sources.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.sources.list", Library, Read, USER_OWNED, true),
    depot!("depot.sources.refresh", Library, Operate, USER_OWNED, true),
    depot!("depot.system.status", Platform, Read, INSTALLATION, false),
    depot!(
        "depot.tokens.create",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!("depot.tokens.list", Platform, Read, INSTALLATION, false),
    depot!("depot.tokens.revoke", Platform, Delete, INSTALLATION, false),
    depot!("depot.uploads.create", Library, Operate, USER_OWNED, true),
    depot!("depot.uploads.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.uploads.get", Library, Read, USER_OWNED, true),
];

pub(crate) fn duplicate_depot_operations() -> BTreeSet<&'static str> {
    let mut seen = BTreeSet::new();
    DEPOT_OPERATIONS
        .iter()
        .filter_map(|operation| (!seen.insert(operation.name)).then_some(operation.name))
        .collect()
}
