use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;

pub(crate) const EXPECTED_ACTIONS: usize = 224;
pub(crate) const EXPECTED_CLI_ACTIONS: usize = 76;
pub(crate) const EXPECTED_MCP_ACTIONS: usize = 223;
pub(crate) const EXPECTED_API_ACTIONS: usize = 221;
pub(crate) const EXPECTED_WEB_ACTIONS: usize = 122;
pub(crate) const EXPECTED_SHARED_CLI_MCP_API_ACTIONS: usize = 76;

const INTENT_JSON: &str = include_str!("../fixtures/action_cases.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaseIntent {
    pub(crate) service: String,
    pub(crate) action: String,
    pub(crate) canonical_action: Option<String>,
    pub(crate) applicable_surfaces: BTreeSet<Surface>,
    pub(crate) applicable_features: BTreeSet<String>,
    pub(crate) scenario_kind: ScenarioKind,
    pub(crate) scenario_id: String,
    pub(crate) minimum_evidence: EvidenceLevel,
    pub(crate) persistence_class: PersistenceClass,
    pub(crate) scenario_owner: ScenarioOwner,
    pub(crate) setup_ref: String,
    pub(crate) fixture_params: FixtureRecipe,
    pub(crate) required: bool,
    pub(crate) exclusion_reason: Option<String>,
}

impl CaseIntent {
    pub(crate) fn key(&self) -> String {
        format!("{}:{}", self.service, self.action)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Surface {
    Cli,
    Mcp,
    Api,
    WebUi,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScenarioKind {
    LiveInvoke,
    ContractProbe,
    StatefulScenario,
    DestructiveIsolated,
    ConditionalOptional,
    ExternalOptional,
    ExcludedWithReason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceLevel {
    MetadataOnly,
    RouterReachable,
    LiveErrorPath,
    LiveSuccess,
    LiveStateTransition,
    LiveRestartPersistence,
    CrossSurfaceParity,
    PackagedArtifactVerified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistenceClass {
    None,
    Ephemeral,
    Reconstructed,
    Durable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScenarioOwner {
    CatalogGovernance,
    SurfaceActionRunner,
    StatefulWorkflowRunner,
    HttpRouteRunner,
    OptionalExternalRunner,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureRecipe {
    pub(crate) fixture: String,
    pub(crate) parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CatalogAction {
    pub(crate) service: String,
    pub(crate) action: String,
    pub(crate) destructive: bool,
    pub(crate) requires_admin: bool,
    pub(crate) required_scopes: Vec<String>,
    pub(crate) surface_availability: SurfaceAvailability,
    pub(crate) requires_http_subject: bool,
    pub(crate) auth_posture: String,
    pub(crate) builtin: bool,
    pub(crate) params: Vec<CatalogParam>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CatalogParam {
    pub(crate) name: String,
    pub(crate) required: bool,
}

impl CatalogAction {
    pub(crate) fn key(&self) -> String {
        format!("{}:{}", self.service, self.action)
    }

    pub(crate) fn surfaces(&self) -> BTreeSet<Surface> {
        let mut surfaces = BTreeSet::new();
        if self.surface_availability.cli {
            surfaces.insert(Surface::Cli);
        }
        if self.surface_availability.mcp {
            surfaces.insert(Surface::Mcp);
        }
        if self.surface_availability.api {
            surfaces.insert(Surface::Api);
        }
        if self.surface_availability.web_ui {
            surfaces.insert(Surface::WebUi);
        }
        surfaces
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SurfaceAvailability {
    pub(crate) cli: bool,
    pub(crate) mcp: bool,
    pub(crate) api: bool,
    pub(crate) web_ui: bool,
}

pub(crate) fn intents() -> &'static [CaseIntent] {
    static INTENTS: OnceLock<Vec<CaseIntent>> = OnceLock::new();
    INTENTS.get_or_init(|| {
        serde_json::from_str(INTENT_JSON).expect("action_cases.json must be valid CaseIntent JSON")
    })
}

pub(crate) fn compiled_shape() -> &'static str {
    if cfg!(feature = "all") {
        "all"
    } else if cfg!(feature = "gateway") {
        "gateway-host"
    } else if cfg!(feature = "fs") {
        "fs"
    } else if cfg!(feature = "skills") {
        "skills"
    } else if cfg!(feature = "lab-admin") {
        "lab-admin"
    } else if cfg!(feature = "api-docs") {
        "api-docs"
    } else {
        "no-default"
    }
}

pub(crate) fn compiled_intents() -> impl Iterator<Item = &'static CaseIntent> {
    let shape = compiled_shape();
    intents()
        .iter()
        .filter(move |intent| intent.applicable_features.contains(shape))
}

pub(crate) fn intent_map() -> Result<BTreeMap<String, &'static CaseIntent>, Vec<String>> {
    intent_map_from(intents())
}

pub(crate) fn intent_map_from(
    values: &[CaseIntent],
) -> Result<BTreeMap<String, &CaseIntent>, Vec<String>> {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    for intent in values {
        let key = intent.key();
        if map.insert(key.clone(), intent).is_some() {
            errors.push(format!("duplicate action intent: {key}"));
        }
    }
    if errors.is_empty() {
        Ok(map)
    } else {
        Err(errors)
    }
}

pub(crate) fn catalog_map(
    catalog: &[CatalogAction],
) -> Result<BTreeMap<String, &CatalogAction>, Vec<String>> {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    for action in catalog {
        let key = action.key();
        if map.insert(key.clone(), action).is_some() {
            errors.push(format!("duplicate catalog action: {key}"));
        }
    }
    if errors.is_empty() {
        Ok(map)
    } else {
        Err(errors)
    }
}

pub(crate) fn validate_intent_shape(intent: &CaseIntent) -> Vec<String> {
    let mut errors = Vec::new();
    let key = intent.key();
    if intent.scenario_id.trim().is_empty() {
        errors.push(format!("{key}: scenario_id is empty"));
    }
    if intent.setup_ref.trim().is_empty() {
        errors.push(format!("{key}: setup_ref is empty"));
    }
    if ![
        "catalog-only",
        "disposable-home",
        "disposable-home+browser-subject",
        "disposable-home+synthetic-external",
    ]
    .contains(&intent.setup_ref.as_str())
    {
        errors.push(format!(
            "{key}: setup_ref is not an approved isolated fixture"
        ));
    }
    if !approved_fixture(&intent.fixture_params.fixture) {
        errors.push(format!("{key}: fixture is not a named hermetic recipe"));
    }
    for (name, value) in &intent.fixture_params.parameters {
        if name.trim().is_empty() || !value.starts_with("$fixture.") {
            errors.push(format!(
                "{key}: parameter {name:?} must use a named fixture token"
            ));
        }
        let lower = value.to_ascii_lowercase();
        if value.starts_with('/')
            || value.contains("..")
            || lower.contains("$home")
            || lower.contains("token")
            || lower.contains("credential")
            || lower.contains("password")
            || lower.contains("secret")
        {
            errors.push(format!(
                "{key}: parameter {name:?} may escape hermetic state"
            ));
        }
    }
    if intent.applicable_surfaces.is_empty() {
        errors.push(format!("{key}: applicable_surfaces is empty"));
    }
    if intent.applicable_features.is_empty() {
        errors.push(format!("{key}: applicable_features is empty"));
    }
    if intent.required
        && matches!(
            intent.scenario_kind,
            ScenarioKind::ConditionalOptional
                | ScenarioKind::ExternalOptional
                | ScenarioKind::ExcludedWithReason
        )
    {
        errors.push(format!(
            "{key}: an optional/excluded case cannot be required"
        ));
    }
    if !intent.required
        && !matches!(
            intent.scenario_kind,
            ScenarioKind::ConditionalOptional
                | ScenarioKind::ExternalOptional
                | ScenarioKind::ExcludedWithReason
        )
    {
        errors.push(format!("{key}: a non-optional case must be required"));
    }
    match intent.scenario_kind {
        ScenarioKind::ExcludedWithReason => {
            if intent
                .exclusion_reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
            {
                errors.push(format!(
                    "{key}: exclusion requires a current-product reason"
                ));
            }
        }
        _ if intent.exclusion_reason.is_some() => {
            errors.push(format!("{key}: only exclusions may carry exclusion_reason"));
        }
        _ => {}
    }
    match intent.scenario_kind {
        ScenarioKind::ContractProbe
            if intent.scenario_owner != ScenarioOwner::CatalogGovernance =>
        {
            errors.push(format!(
                "{key}: contract probe has the wrong scenario owner"
            ));
        }
        ScenarioKind::LiveInvoke if intent.scenario_owner != ScenarioOwner::SurfaceActionRunner => {
            errors.push(format!(
                "{key}: live invocation has the wrong scenario owner"
            ));
        }
        ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated
            if intent.scenario_owner != ScenarioOwner::StatefulWorkflowRunner =>
        {
            errors.push(format!("{key}: stateful case has the wrong scenario owner"));
        }
        ScenarioKind::ConditionalOptional
            if intent.scenario_owner != ScenarioOwner::HttpRouteRunner =>
        {
            errors.push(format!(
                "{key}: conditional case has the wrong scenario owner"
            ));
        }
        ScenarioKind::ExternalOptional
            if intent.scenario_owner != ScenarioOwner::OptionalExternalRunner =>
        {
            errors.push(format!("{key}: external case has the wrong scenario owner"));
        }
        _ => {}
    }
    if matches!(
        intent.scenario_kind,
        ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated
    ) && intent.persistence_class != PersistenceClass::Durable
    {
        errors.push(format!(
            "{key}: stateful workflow must declare durable persistence"
        ));
    }
    if intent.fixture_params.fixture == "synthetic_external"
        && intent.scenario_kind != ScenarioKind::ExternalOptional
    {
        errors.push(format!(
            "{key}: only optional external cases may use synthetic external state"
        ));
    }
    if intent.fixture_params.fixture == "fs_browser_subject"
        && !(intent.service == "fs" && intent.action == "fs.preview")
    {
        errors.push(format!(
            "{key}: browser-subject fixture is limited to fs.preview"
        ));
    }
    errors
}

fn approved_fixture(name: &str) -> bool {
    if matches!(
        name,
        "catalog_metadata" | "fs_browser_subject" | "synthetic_external"
    ) {
        return true;
    }
    let Some(rest) = name.strip_prefix("isolated_") else {
        return false;
    };
    let Some((service, purpose)) = rest.rsplit_once('_') else {
        return false;
    };
    matches!(
        service,
        "artifacts"
            | "browser"
            | "bundles"
            | "doctor"
            | "fs"
            | "gateway"
            | "jobs"
            | "lab_admin"
            | "server_logs"
            | "setup"
            | "skills"
            | "snippets"
            | "sources"
            | "stash"
            | "uploads"
    ) && matches!(purpose, "readonly" | "workflow" | "destructive")
}

/// Execution history is generated by runners and must never be committed as intent.
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct CaseOutcome {
    pub(crate) key: String,
    pub(crate) status: OutcomeStatus,
    pub(crate) achieved_evidence: EvidenceLevel,
    pub(crate) timing_ms: u64,
    pub(crate) failure_class: Option<String>,
    pub(crate) cleanup_ok: bool,
    pub(crate) artifacts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum OutcomeStatus {
    Passed,
    Failed,
    Skipped,
}

#[allow(dead_code)]
pub(crate) fn outcome_satisfies(intent: &CaseIntent, outcome: &CaseOutcome) -> bool {
    outcome.key == intent.key()
        && outcome.status == OutcomeStatus::Passed
        && outcome.achieved_evidence >= intent.minimum_evidence
        && outcome.cleanup_ok
}
