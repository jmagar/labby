use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use labby_apis::core::{EnvVar, PluginMeta};
use serde::Deserialize;

use super::routes::{build_route_docs, service_has_action_api_route};
use super::types::{
    DocsProjection, EnvDoc, FeatureClass, FeatureDoc, FeatureMatrix, FeatureMismatch, ServiceDoc,
    ServiceExposure, SurfaceAvailability,
};
use crate::api::openapi::build_openapi_spec;
use crate::catalog::build_catalog;
use crate::registry::{RegisteredService, build_docs_registry};

const LABBY_CRATE: &str = "labby";
const LABBY_APIS_CRATE: &str = "labby-apis";
const LABBY_APIS_PREFIX: &str = "labby-apis/";

pub fn build_docs_projection(repo_root: &Path) -> Result<DocsProjection> {
    let registry = build_docs_registry();
    let mcp_help = build_catalog(&registry);
    let services = registry.services();
    let feature_matrix = build_feature_matrix(repo_root)?;
    let service_catalog = build_service_catalog(services, &feature_matrix, repo_root);
    let env_reference = build_env_reference(&service_catalog);
    let action_catalog = super::action_catalog::build_action_catalog(services);
    let api_route_services = service_catalog
        .iter()
        .filter(|service| service.surfaces.api && service_has_action_api_route(&service.name))
        .map(|service| service.name.clone())
        .collect::<Vec<_>>();
    let api_routes = build_route_docs(&api_route_services);
    let openapi_json =
        Arc::unwrap_or_clone(build_openapi_spec(services).context("failed to build OpenAPI spec")?);
    Ok(DocsProjection {
        mcp_help,
        service_catalog,
        env_reference,
        action_catalog,
        feature_matrix,
        api_routes,
        openapi_json,
    })
}

fn build_service_catalog(
    services: &[RegisteredService],
    feature_matrix: &FeatureMatrix,
    repo_root: &Path,
) -> Vec<ServiceDoc> {
    let mut docs = services
        .iter()
        .map(|service| service_doc(service, feature_matrix, repo_root))
        .collect::<Vec<_>>();

    for meta in sdk_only_metas() {
        if docs.iter().any(|service| service.name == meta.name) {
            continue;
        }
        docs.push(ServiceDoc {
            name: meta.name.to_string(),
            display_name: meta.display_name.to_string(),
            description: meta.description.to_string(),
            category: meta.category.as_str().to_string(),
            status: "sdk_only".to_string(),
            feature: sdk_only_feature(meta),
            exposure: ServiceExposure::SdkOnly,
            surfaces: SurfaceAvailability::none(),
            default_port: meta.default_port,
            docs_url: non_empty(meta.docs_url),
            coverage_doc: doc_exists(repo_root, &format!("docs/coverage/{}.md", meta.name)),
            upstream_doc: doc_exists(repo_root, &format!("docs/upstream-api/{}.md", meta.name)),
            supports_multi_instance: meta.supports_multi_instance,
            metadata_source: "PluginMeta only".to_string(),
        });
    }

    docs.sort_by(|a, b| a.name.cmp(&b.name));
    docs
}

fn sdk_only_feature(meta: &PluginMeta) -> Option<String> {
    match meta.name {
        "mcpregistry" => Some("marketplace".to_string()),
        _ => Some(meta.name.to_string()),
    }
}

fn service_doc(
    service: &RegisteredService,
    feature_matrix: &FeatureMatrix,
    repo_root: &Path,
) -> ServiceDoc {
    let meta = meta_for(service.name);
    let feature = service_feature(service.name, feature_matrix);
    let exposure = if service.name == "lab_admin" {
        ServiceExposure::RuntimeConditional
    } else if feature.is_some() {
        ServiceExposure::FeatureGated
    } else {
        ServiceExposure::AlwaysOn
    };
    let display_name = meta.map_or_else(
        || service.name.to_string(),
        |meta| meta.display_name.to_string(),
    );
    let description = meta.map_or(service.description, |meta| meta.description);
    let category = meta.map_or(service.category, |meta| meta.category.as_str());

    ServiceDoc {
        name: service.name.to_string(),
        display_name,
        description: description.to_string(),
        category: category.to_string(),
        status: service.status.to_string(),
        feature,
        exposure,
        surfaces: service_surfaces(service.name),
        default_port: meta.and_then(|meta| meta.default_port),
        docs_url: meta.and_then(|meta| non_empty(meta.docs_url)),
        coverage_doc: doc_exists(repo_root, &format!("docs/coverage/{}.md", service.name)),
        upstream_doc: doc_exists(repo_root, &format!("docs/upstream-api/{}.md", service.name)),
        supports_multi_instance: meta.is_some_and(|meta| meta.supports_multi_instance),
        metadata_source: if meta.is_some() {
            "registry + PluginMeta".to_string()
        } else {
            "registry synthetic metadata".to_string()
        },
    }
}

fn build_env_reference(services: &[ServiceDoc]) -> Vec<EnvDoc> {
    let mut vars = Vec::new();
    for service in services {
        let Some(meta) = meta_for(&service.name) else {
            continue;
        };
        vars.extend(env_docs(
            service,
            meta.required_env,
            true,
            meta.default_port,
        ));
        vars.extend(env_docs(
            service,
            meta.optional_env,
            false,
            meta.default_port,
        ));
    }
    vars.sort_by(|a, b| {
        (a.service.as_str(), a.env_var.as_str()).cmp(&(b.service.as_str(), b.env_var.as_str()))
    });
    vars
}

fn env_docs(
    service: &ServiceDoc,
    envs: &[EnvVar],
    required: bool,
    default_port: Option<u16>,
) -> Vec<EnvDoc> {
    envs.iter()
        .map(|env| EnvDoc {
            service: service.name.clone(),
            env_var: env.name.to_string(),
            required,
            secret: env.secret,
            description: env.description.to_string(),
            example: sanitized_example(env),
            default_port,
        })
        .collect()
}

fn build_feature_matrix(repo_root: &Path) -> Result<FeatureMatrix> {
    let lab = read_manifest(&repo_root.join("crates/labby/Cargo.toml"))?;
    let apis = read_manifest(&repo_root.join("crates/labby-apis/Cargo.toml"))?;
    let lab_features = lab.features;
    let api_features = apis.features;
    let lab_all = feature_set(&lab_features, "all");
    let api_all = feature_set(&api_features, "all");
    let lab_default = feature_set(&lab_features, "default");
    let api_default = feature_set(&api_features, "default");
    let mut features = Vec::new();
    let mut mismatches = Vec::new();

    for (feature, deps) in &lab_features {
        let classification = classify_lab_feature(feature, deps, &api_features);
        let mapped = mapped_lab_feature(feature, deps, &api_features);
        if classification == FeatureClass::ServicePassthrough {
            if !api_features.contains_key(feature.as_str()) {
                mismatches.push(FeatureMismatch {
                    feature: feature.clone(),
                    message: "service passthrough missing matching labby-apis feature".to_string(),
                });
            }
            if !lab_all.contains(feature.as_str()) {
                mismatches.push(FeatureMismatch {
                    feature: feature.clone(),
                    message: "service feature missing from labby all".to_string(),
                });
            }
            if !api_all.contains(feature.as_str()) {
                mismatches.push(FeatureMismatch {
                    feature: feature.clone(),
                    message: "service feature missing from labby-apis all".to_string(),
                });
            }
        }
        features.push(FeatureDoc {
            crate_name: LABBY_CRATE.to_string(),
            feature: feature.clone(),
            dependencies: deps.clone(),
            included_in_default: lab_default.contains(feature.as_str()),
            included_in_all: lab_all.contains(feature.as_str()),
            classification,
            mapped_crate_feature: mapped,
            exception_reason: exception_reason(classification).map(str::to_string),
        });
    }

    for (feature, deps) in &api_features {
        let classification = classify_api_feature(feature, &lab_features);
        if classification == FeatureClass::SdkOnly && !api_all.contains(feature.as_str()) {
            mismatches.push(FeatureMismatch {
                feature: feature.clone(),
                message: "SDK-only service feature missing from labby-apis all".to_string(),
            });
        }
        features.push(FeatureDoc {
            crate_name: LABBY_APIS_CRATE.to_string(),
            feature: feature.clone(),
            dependencies: deps.clone(),
            included_in_default: api_default.contains(feature.as_str()),
            included_in_all: api_all.contains(feature.as_str()),
            classification,
            mapped_crate_feature: lab_features
                .contains_key(feature.as_str())
                .then(|| format!("{LABBY_CRATE}/{feature}")),
            exception_reason: exception_reason(classification).map(str::to_string),
        });
    }

    features.sort_by(|a, b| {
        (a.crate_name.as_str(), a.feature.as_str())
            .cmp(&(b.crate_name.as_str(), b.feature.as_str()))
    });
    mismatches.sort_by(|a, b| a.feature.cmp(&b.feature));
    Ok(FeatureMatrix {
        features,
        mismatches,
    })
}

fn read_manifest(path: &Path) -> Result<CargoManifest> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

#[derive(Debug, Deserialize)]
struct CargoManifest {
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

fn feature_set(features: &BTreeMap<String, Vec<String>>, name: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_feature_set(features, name, &mut out, &mut BTreeSet::new());
    out
}

fn collect_feature_set(
    features: &BTreeMap<String, Vec<String>>,
    name: &str,
    out: &mut BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) {
    if !seen.insert(name.to_string()) {
        return;
    }

    for dep in features.get(name).into_iter().flatten() {
        let normalized = dep.strip_prefix(LABBY_APIS_PREFIX).unwrap_or(dep);
        out.insert(normalized.to_string());
        if features.contains_key(normalized) {
            collect_feature_set(features, normalized, out, seen);
        }
    }
}

fn classify_lab_feature(
    feature: &str,
    deps: &[String],
    api_features: &BTreeMap<String, Vec<String>>,
) -> FeatureClass {
    if matches!(feature, "all" | "default") {
        FeatureClass::AggregateDefault
    } else if matches!(feature, "gateway" | "marketplace" | "fs" | "lab-admin") {
        FeatureClass::ProductSlice
    } else if matches!(feature, "node-runtime") {
        FeatureClass::BinaryOnly
    } else if deps
        .iter()
        .any(|dep| dep == &format!("{LABBY_APIS_PREFIX}{feature}"))
        && api_features.contains_key(feature)
    {
        FeatureClass::ServicePassthrough
    } else if deps.iter().any(|dep| dep.starts_with("dep:")) {
        FeatureClass::HelperInternal
    } else {
        FeatureClass::IntentionalException
    }
}

fn classify_api_feature(
    feature: &str,
    lab_features: &BTreeMap<String, Vec<String>>,
) -> FeatureClass {
    if matches!(feature, "all" | "default") {
        FeatureClass::AggregateDefault
    } else if matches!(feature, "servarr" | "test-utils") {
        FeatureClass::HelperInternal
    } else if lab_features.contains_key(feature) {
        FeatureClass::ServicePassthrough
    } else {
        FeatureClass::SdkOnly
    }
}

fn mapped_lab_feature(
    feature: &str,
    deps: &[String],
    api_features: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    deps.iter()
        .filter_map(|dep| dep.strip_prefix(LABBY_APIS_PREFIX))
        .find(|dep| api_features.contains_key(*dep))
        .map(|dep| format!("{LABBY_APIS_PREFIX}{dep}"))
        .or_else(|| {
            api_features
                .contains_key(feature)
                .then(|| format!("{LABBY_APIS_PREFIX}{feature}"))
        })
}

fn exception_reason(classification: FeatureClass) -> Option<&'static str> {
    match classification {
        FeatureClass::ProductSlice => Some("standalone product slice"),
        FeatureClass::BinaryOnly => Some("binary-only Lab feature"),
        FeatureClass::HelperInternal => Some("helper/internal feature"),
        FeatureClass::AggregateDefault => Some("aggregate/default feature"),
        FeatureClass::IntentionalException => Some("intentional crate-local exception"),
        FeatureClass::ServicePassthrough | FeatureClass::SdkOnly => None,
    }
}

fn service_feature(service: &str, matrix: &FeatureMatrix) -> Option<String> {
    matrix
        .features
        .iter()
        .find(|feature| {
            feature.crate_name == "labby"
                && feature.feature == service
                && matches!(
                    feature.classification,
                    FeatureClass::ServicePassthrough
                        | FeatureClass::ProductSlice
                        | FeatureClass::BinaryOnly
                )
        })
        .map(|feature| feature.feature.clone())
}

pub(super) fn service_surfaces(service: &str) -> SurfaceAvailability {
    SurfaceAvailability {
        cli: !matches!(service, "device" | "fs"),
        mcp: true,
        api: service_has_action_api_route(service)
            || matches!(service, "device" | "marketplace" | "doctor" | "setup"),
        web_ui: matches!(
            service,
            "gateway" | "marketplace" | "logs" | "setup" | "device" | "fs"
        ),
    }
}

impl SurfaceAvailability {
    fn none() -> Self {
        Self {
            cli: false,
            mcp: false,
            api: false,
            web_ui: false,
        }
    }
}

fn doc_exists(repo_root: &Path, rel: &str) -> Option<String> {
    repo_root.join(rel).exists().then(|| rel.to_string())
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn sanitized_example(env: &EnvVar) -> String {
    if env.secret {
        format!("<{}>", env.name.to_ascii_lowercase())
    } else {
        env.example.to_string()
    }
}

#[cfg(test)]
pub(crate) fn secret_example_is_suspicious(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("cookie")
        || lower.starts_with("sk-")
        || lower.starts_with("eyj")
        || lower.contains("-----begin ")
        || trimmed.len() >= 20
}

fn sdk_only_metas() -> Vec<&'static PluginMeta> {
    vec![
        #[cfg(feature = "acp_registry")]
        &labby_apis::acp_registry::META,
        #[cfg(feature = "marketplace")]
        &labby_apis::mcpregistry::META,
    ]
}

#[allow(clippy::too_many_lines)]
fn meta_for(name: &str) -> Option<&'static PluginMeta> {
    match name {
        "marketplace" => Some(&labby_apis::marketplace::META),
        "doctor" => Some(&labby_apis::doctor::META),
        "setup" => Some(&labby_apis::setup::META),
        "stash" => Some(&labby_apis::stash::META),
        "acp" => Some(&labby_apis::acp::META),
        "device_runtime" => Some(&labby_apis::device_runtime::META),
        #[cfg(feature = "deploy")]
        "deploy" => Some(&labby_apis::deploy::META),
        _ => None,
    }
}

#[allow(dead_code)]
pub fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("cannot determine workspace root from CARGO_MANIFEST_DIR"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_examples_are_always_placeholdered() {
        let env = EnvVar {
            name: "SERVICE_API_KEY",
            description: "API key",
            example: "demo-key",
            secret: true,
            ui: None,
        };
        assert_eq!(sanitized_example(&env), "<service_api_key>");
    }

    #[test]
    fn action_catalog_exposes_fs_preview_as_http_only() {
        let projection = build_docs_projection(&workspace_root().unwrap()).unwrap();
        let preview = projection
            .action_catalog
            .iter()
            .find(|action| action.service == "fs" && action.action == "fs.preview")
            .unwrap();
        assert!(preview.surface_availability.api);
        assert!(preview.surface_availability.web_ui);
        assert!(!preview.surface_availability.mcp);
        assert!(preview.requires_http_subject);
    }

    #[test]
    fn mcp_help_is_equivalent_to_mcp_action_projection() {
        let projection = build_docs_projection(&workspace_root().unwrap()).unwrap();
        let help_actions = projection
            .mcp_help
            .services
            .iter()
            .flat_map(|service| {
                service
                    .actions
                    .iter()
                    .map(|action| (service.name.as_str(), action.name.as_str()))
            })
            .collect::<BTreeSet<_>>();
        let projected_mcp_actions = projection
            .action_catalog
            .iter()
            .filter(|action| action.surface_availability.mcp && !action.builtin)
            .map(|action| (action.service.as_str(), action.action.as_str()))
            .collect::<BTreeSet<_>>();
        assert_eq!(help_actions, projected_mcp_actions);
    }
}
