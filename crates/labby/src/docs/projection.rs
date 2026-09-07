use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
#[cfg(feature = "api-docs")]
use std::sync::Arc;

use anyhow::{Context, Result};
use labby_primitives::plugin::{EnvVar, PluginMeta};
use serde::Deserialize;

use super::routes::{build_route_docs, service_has_action_api_route};
use super::types::{
    ConfigDoc, DocsProjection, EnvDoc, FeatureClass, FeatureDoc, FeatureMatrix, FeatureMismatch,
    ServiceDoc, ServiceExposure, SurfaceAvailability,
};
use crate::catalog::build_catalog;
use crate::registry::{RegisteredService, build_docs_registry};

#[cfg(feature = "api-docs")]
use crate::api::openapi::build_openapi_spec;

const LABBY_CRATE: &str = "labby";
const LABBY_APIS_CRATE: &str = "labby-apis";
const LABBY_APIS_PREFIX: &str = "labby-apis/";
const EXTRACTED_FEATURE_CRATES: &[&str] = &["labby-auth", "labby-runtime"];
const EXTRACTED_FEATURELESS_CRATES: &[&str] = &[
    "labby-codemode",
    "labby-gateway",
    "labby-web",
    "labby-winjob",
];

pub fn build_docs_projection(repo_root: &Path) -> Result<DocsProjection> {
    let registry = build_docs_registry();
    let mcp_help = build_catalog(&registry);
    let services = registry.services();
    let feature_matrix = build_feature_matrix(repo_root)?;
    let service_catalog = build_service_catalog(services, &feature_matrix);
    let proxy_config_reference = build_proxy_config_reference();
    let env_reference = build_env_reference(&service_catalog);
    let action_catalog = super::action_catalog::build_action_catalog(services);
    let api_route_services = service_catalog
        .iter()
        .filter(|service| service.surfaces.api && service_has_action_api_route(&service.name))
        .map(|service| service.name.clone())
        .collect::<Vec<_>>();
    let api_routes = build_route_docs(&api_route_services);
    #[cfg(feature = "api-docs")]
    let openapi_json =
        Arc::unwrap_or_clone(build_openapi_spec(services).context("failed to build OpenAPI spec")?);
    #[cfg(not(feature = "api-docs"))]
    let openapi_json = String::new();
    Ok(DocsProjection {
        mcp_help,
        service_catalog,
        proxy_config_reference,
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
) -> Vec<ServiceDoc> {
    let mut docs = services
        .iter()
        .map(|service| service_doc(service, feature_matrix))
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
            supports_multi_instance: meta.supports_multi_instance,
            metadata_source: "PluginMeta only".to_string(),
        });
    }

    if !docs.iter().any(|service| service.name == "proxy") {
        docs.push(ServiceDoc {
            name: "proxy".to_string(),
            display_name: "Stdio MCP Proxy".to_string(),
            description:
                "Expose one explicitly selected stdio MCP server as faithful Streamable HTTP"
                    .to_string(),
            category: "bootstrap".to_string(),
            status: "available".to_string(),
            feature: Some("gateway".to_string()),
            exposure: ServiceExposure::FeatureGated,
            surfaces: SurfaceAvailability {
                cli: true,
                mcp: false,
                api: false,
                web_ui: false,
            },
            default_port: None,
            docs_url: Some("docs/guides/STDIO_MCP_PROXY.md".to_string()),
            supports_multi_instance: false,
            metadata_source: "CLI runtime + ProxyPreferences".to_string(),
        });
    }

    docs.sort_by(|a, b| a.name.cmp(&b.name));
    docs
}

fn sdk_only_feature(meta: &PluginMeta) -> Option<String> {
    Some(meta.name.to_string())
}

fn service_doc(service: &RegisteredService, feature_matrix: &FeatureMatrix) -> ServiceDoc {
    let meta = meta_for(service.name);
    let feature = service_feature(service.name, feature_matrix);
    let exposure = if matches!(service.name, "lab_admin" | "stash") {
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
    vars.extend([
        core_env("LABBY_MCP_HTTP_HOST", false, false, "127.0.0.1", "HTTP MCP bind host"),
        core_env("LABBY_MCP_HTTP_PORT", false, false, "8765", "HTTP MCP bind port"),
        core_env("LABBY_LOG", false, false, "labby=info,labby_apis=warn", "Tracing filter directive"),
        core_env("LABBY_LOG_FORMAT", false, false, "json", "Tracing output format"),
        core_env("LABBY_RECOVERY_KEY_PATH", true, true, "/secure/labby-recovery.key", "External HMAC key for durable-state export, verification, and restore"),
        core_env("LABBY_MCP_GATEWAY_URL", false, false, "https://mcp.example.com", "Canonical public MCP gateway URL"),
        auth_env("LABBY_AUTH_MODE", false, false, "bearer", "Inbound authentication mode: bearer or oauth"),
        auth_env("LABBY_PUBLIC_URL", true, false, "https://lab.example.com", "Canonical public application URL and OAuth issuer"),
        auth_env("LABBY_GOOGLE_CLIENT_ID", true, false, "google-client-id", "Google OAuth client identifier used in oauth mode"),
        auth_env("LABBY_GOOGLE_CLIENT_SECRET", true, true, "<labby_google_client_secret>", "Google OAuth client secret used in oauth mode"),
        auth_env("LABBY_AUTH_PROVIDER", false, false, "authelia", "Active inbound identity provider: google or authelia"),
        auth_env("LABBY_AUTHELIA_ISSUER_URL", false, false, "https://auth.example.com", "Exact Authelia OIDC issuer URL"),
        auth_env("LABBY_AUTHELIA_CLIENT_ID", false, false, "labby", "Authelia confidential OIDC client identifier"),
        auth_env("LABBY_AUTHELIA_CLIENT_SECRET", false, true, "<labby_authelia_client_secret>", "Authelia confidential OIDC client secret"),
        auth_env("LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN", false, false, "https://auth.example.com", "Exact HTTPS private issuer origin explicitly trusted by the operator"),
        auth_env("LABBY_AUTHELIA_CA_CERT_PATH", false, false, "/etc/labby/authelia-ca.pem", "PEM CA certificate trusted only for the exact Authelia issuer origin"),
        auth_env("LABBY_AUTH_ADMIN_EMAIL", true, false, "admin@example.com", "Bootstrap administrator email required in oauth mode"),
        auth_env("LABBY_AUTH_ALLOWED_REDIRECT_URIS", false, false, "https://chatgpt.com/connector/oauth/*", "Comma-separated exact or wildcard OAuth redirect allowlist"),
        auth_env("LABBY_AUTH_ALLOWED_EMAIL_DOMAINS", false, false, "example.com", "Comma-separated Google Workspace hosted-domain allowlist"),
        auth_env("LABBY_AUTH_SQLITE_PATH", false, false, "~/.labby/auth.db", "OAuth authorization-state SQLite database path"),
        auth_env("LABBY_AUTH_KEY_PATH", false, true, "~/.labby/auth-jwt.pem", "OAuth JWT signing-key path"),
        auth_env("LABBY_MCP_HTTP_TOKEN", false, true, "<labby_mcp_http_token>", "Static bearer token for protected HTTP routes in bearer mode"),
        auth_env("LABBY_TOKEN_ENCRYPTION_KEY", true, true, "<64-hex-or-base64url-key>", "Encryption key for persisted provider access and refresh tokens"),
        auth_env("LABBY_GOOGLE_CALLBACK_URL", false, false, "https://lab.example.com/auth/google/callback", "Absolute Google OAuth callback URL override"),
        auth_env("LABBY_GOOGLE_CALLBACK_PATH", false, false, "/auth/google/callback", "Google OAuth callback path"),
        auth_env("LABBY_GOOGLE_SCOPES", false, false, "openid,email,profile", "Comma-separated Google OAuth scopes"),
        auth_env("LABBY_AUTH_ACCESS_TOKEN_TTL_SECS", false, false, "3600", "Labby access-token lifetime in seconds"),
        auth_env("LABBY_AUTH_REFRESH_TOKEN_TTL_SECS", false, false, "2592000", "Labby refresh-token lifetime in seconds"),
        auth_env("LABBY_AUTH_CODE_TTL_SECS", false, false, "300", "Authorization-code lifetime in seconds"),
        auth_env("LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE", false, false, "20", "Per-IP dynamic-client-registration rate limit"),
        auth_env("LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE", false, false, "60", "Per-IP authorization and browser-login rate limit"),
        auth_env("LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE", false, false, "120", "Per-IP token and revocation endpoint rate limit"),
        auth_env("LABBY_AUTH_MAX_PENDING_OAUTH_STATES", false, false, "1024", "Maximum non-expired pending OAuth states"),
        auth_env("LABBY_AUTH_SCOPES_SUPPORTED", false, false, "lab:read,lab,lab:admin", "Comma-separated scopes advertised and accepted for the canonical protected resource"),
        auth_env("LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY", false, false, "false", "Temporary compatibility switch for clients without RFC 9207 response issuer support"),
        auth_env("LABBY_AUTH_MACHINE_CLIENTS_JSON", false, true, "[]", "Preregistered machine-client definitions"),
        auth_env("LABBY_AUTH_ENTERPRISE_ISSUERS_JSON", false, true, "[]", "Trusted enterprise issuer definitions"),
        EnvDoc {
            service: "proxy".to_string(),
            env_var: "LABBY_PROXY_BEARER_TOKEN".to_string(),
            required: false,
            secret: true,
            description: "Default static bearer secret; the key name may be changed with proxy.bearer_token_env"
                .to_string(),
            example: "<labby_proxy_bearer_token>".to_string(),
            default_port: None,
        },
        EnvDoc {
            service: "proxy".to_string(),
            env_var: "LABBY_TAILSCALE_BIN".to_string(),
            required: false,
            secret: false,
            description: "Override the Tailscale CLI executable used by proxy publication and preflight"
                .to_string(),
            example: "tailscale".to_string(),
            default_port: None,
        },
        EnvDoc {
            service: "proxy".to_string(),
            env_var: "LABBY_GW_UPSTREAM_STDERR".to_string(),
            required: false,
            secret: false,
            description: "Set forwarding level for the proxied stdio child's redacted stderr; default debug"
                .to_string(),
            example: "debug".to_string(),
            default_port: None,
        },
        EnvDoc {
            service: "proxy".to_string(),
            env_var: "LABBY_PROXY_TEST_RENEW_MS".to_string(),
            required: false,
            secret: false,
            description: "Test-only OAuth lease renewal override, compiled only with proxy-testkit"
                .to_string(),
            example: "100".to_string(),
            default_port: None,
        },
    ]);
    vars.sort_by(|a, b| {
        (a.service.as_str(), a.env_var.as_str()).cmp(&(b.service.as_str(), b.env_var.as_str()))
    });
    vars
}

fn auth_env(name: &str, required: bool, secret: bool, example: &str, description: &str) -> EnvDoc {
    EnvDoc {
        service: "auth".to_string(),
        env_var: name.to_string(),
        required,
        secret,
        description: description.to_string(),
        example: example.to_string(),
        default_port: None,
    }
}

fn core_env(name: &str, required: bool, secret: bool, example: &str, description: &str) -> EnvDoc {
    EnvDoc {
        service: "lab".to_string(),
        env_var: name.to_string(),
        required,
        secret,
        description: description.to_string(),
        example: example.to_string(),
        default_port: None,
    }
}

fn build_proxy_config_reference() -> Vec<ConfigDoc> {
    let entries = [
        (
            "exposure",
            "tailscale|local",
            "tailscale",
            None,
            "Publication controller",
        ),
        (
            "auth",
            "tailnet|bearer|oauth|none",
            "tailnet",
            None,
            "Authentication policy",
        ),
        (
            "path",
            "absolute non-root path",
            "/mcp",
            None,
            "Public MCP endpoint path",
        ),
        (
            "port",
            "random|u16",
            "random",
            None,
            "External Tailscale HTTPS port selection",
        ),
        (
            "port_range_start",
            "u16",
            "49152",
            None,
            "First random external-port candidate",
        ),
        (
            "port_range_end",
            "u16",
            "65535",
            None,
            "Last random external-port candidate",
        ),
        (
            "bearer_token_env",
            "environment variable name",
            "LABBY_PROXY_BEARER_TOKEN",
            Some("LABBY_PROXY_BEARER_TOKEN"),
            "Environment key containing the static bearer secret",
        ),
        (
            "oauth_scopes",
            "string[]",
            "[mcp:read, mcp:write]",
            None,
            "Scopes required by the exact OAuth resource lease",
        ),
        (
            "inherit_env",
            "environment variable name[]",
            "[]",
            None,
            "Additional ambient variables inherited by the scrubbed child",
        ),
        (
            "shutdown_grace_ms",
            "u64 (1..=60000)",
            "3000",
            None,
            "Grace period preference for supervised shutdown",
        ),
    ];
    entries
        .into_iter()
        .map(|(key, ty, default, env_override, description)| ConfigDoc {
            section: "proxy".to_string(),
            key: key.to_string(),
            toml_path: format!("proxy.{key}"),
            ty: ty.to_string(),
            default: default.to_string(),
            secret: false,
            env_override: env_override.map(str::to_string),
            description: description.to_string(),
        })
        .collect()
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
    let lab_dependencies = lab.dependencies;
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
        let mapped = mapped_lab_feature(deps, &api_features);
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

    for crate_name in EXTRACTED_FEATURE_CRATES {
        let manifest = read_manifest(&repo_root.join(format!("crates/{crate_name}/Cargo.toml")))?;
        let default_active = dependency_active_features(
            &lab_dependencies,
            crate_name,
            &manifest.features,
            &lab_default,
        );
        let all_active =
            dependency_active_features(&lab_dependencies, crate_name, &manifest.features, &lab_all);
        push_extracted_crate_features(
            crate_name,
            manifest.features,
            Some((&default_active, &all_active)),
            &mut features,
        );
    }
    for crate_name in EXTRACTED_FEATURELESS_CRATES {
        let manifest = read_manifest(&repo_root.join(format!("crates/{crate_name}/Cargo.toml")))?;
        let default_active = dependency_active_features(
            &lab_dependencies,
            crate_name,
            &manifest.features,
            &lab_default,
        );
        let all_active =
            dependency_active_features(&lab_dependencies, crate_name, &manifest.features, &lab_all);
        push_extracted_featureless_crate(
            crate_name,
            manifest.features,
            dependency_is_active(&lab_dependencies, crate_name, &lab_default),
            dependency_is_active(&lab_dependencies, crate_name, &lab_all),
            Some((&default_active, &all_active)),
            &mut features,
        );
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

fn push_extracted_crate_features(
    crate_name: &str,
    crate_features: BTreeMap<String, Vec<String>>,
    product_active: Option<(&BTreeSet<String>, &BTreeSet<String>)>,
    features: &mut Vec<FeatureDoc>,
) {
    let default = feature_set(&crate_features, "default");
    let all = feature_set(&crate_features, "all");
    for (feature, deps) in crate_features {
        let classification = if matches!(feature.as_str(), "all" | "default") {
            FeatureClass::AggregateDefault
        } else {
            FeatureClass::ExtractedCrate
        };
        features.push(FeatureDoc {
            crate_name: crate_name.to_string(),
            feature: feature.clone(),
            dependencies: deps,
            included_in_default: product_active.map_or_else(
                || default.contains(feature.as_str()),
                |(active, _)| active.contains(feature.as_str()),
            ),
            included_in_all: product_active.map_or_else(
                || all.contains(feature.as_str()),
                |(_, active)| active.contains(feature.as_str()),
            ),
            classification,
            mapped_crate_feature: None,
            exception_reason: exception_reason(classification).map(str::to_string),
        });
    }
}

fn push_extracted_featureless_crate(
    crate_name: &str,
    crate_features: BTreeMap<String, Vec<String>>,
    included_in_default: bool,
    included_in_all: bool,
    product_active: Option<(&BTreeSet<String>, &BTreeSet<String>)>,
    features: &mut Vec<FeatureDoc>,
) {
    if crate_features.is_empty() {
        features.push(FeatureDoc {
            crate_name: crate_name.to_string(),
            feature: "no_features".to_string(),
            dependencies: Vec::new(),
            included_in_default,
            included_in_all,
            classification: FeatureClass::ExtractedCrate,
            mapped_crate_feature: None,
            exception_reason: Some("extracted crate has no Cargo features".to_string()),
        });
    } else {
        push_extracted_crate_features(crate_name, crate_features, product_active, features);
    }
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
    #[serde(default)]
    dependencies: BTreeMap<String, CargoDependency>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CargoDependency {
    Version(String),
    Detailed {
        #[serde(default)]
        features: Vec<String>,
        #[serde(default)]
        optional: bool,
        #[serde(default = "default_true", rename = "default-features")]
        default_features: bool,
    },
}

fn default_true() -> bool {
    true
}

fn dependency_is_active(
    dependencies: &BTreeMap<String, CargoDependency>,
    crate_name: &str,
    product_features: &BTreeSet<String>,
) -> bool {
    dependencies.get(crate_name).is_some_and(|dependency| {
        !dependency.is_optional() || product_features.contains(&format!("dep:{crate_name}"))
    })
}

fn dependency_active_features(
    dependencies: &BTreeMap<String, CargoDependency>,
    crate_name: &str,
    crate_features: &BTreeMap<String, Vec<String>>,
    product_features: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut active = BTreeSet::new();
    let Some(dependency) = dependencies.get(crate_name) else {
        return active;
    };
    if !dependency_is_active(dependencies, crate_name, product_features) {
        return active;
    }
    if dependency.default_features() && crate_features.contains_key("default") {
        insert_feature_closure(crate_features, "default", &mut active);
    }
    for feature in dependency.features() {
        insert_feature_closure(crate_features, feature, &mut active);
    }
    active
}

fn insert_feature_closure(
    features: &BTreeMap<String, Vec<String>>,
    name: &str,
    out: &mut BTreeSet<String>,
) {
    out.insert(name.to_string());
    out.extend(feature_set(features, name));
}

impl CargoDependency {
    fn features(&self) -> &[String] {
        match self {
            Self::Version(version) => {
                let _ = version;
                &[]
            }
            Self::Detailed { features, .. } => features,
        }
    }

    fn is_optional(&self) -> bool {
        match self {
            Self::Version(version) => {
                let _ = version;
                false
            }
            Self::Detailed { optional, .. } => *optional,
        }
    }

    fn default_features(&self) -> bool {
        match self {
            Self::Version(version) => {
                let _ = version;
                true
            }
            Self::Detailed {
                default_features, ..
            } => *default_features,
        }
    }
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

/// Feature-gated base capabilities: members of `all` that a gateway-only
/// build excludes. Not standalone product slices — classified explicitly so
/// the feature matrix labels them meaningfully instead of falling through to
/// HelperInternal (`acp` carries a `dep:` entry) or IntentionalException
/// (`nodes`/`stash` have empty dependency lists).
const BASE_CAPABILITIES: &[&str] = &["acp", "nodes", "stash"];

fn classify_lab_feature(
    feature: &str,
    deps: &[String],
    api_features: &BTreeMap<String, Vec<String>>,
) -> FeatureClass {
    if matches!(feature, "all" | "default") {
        FeatureClass::AggregateDefault
    } else if BASE_CAPABILITIES.contains(&feature) {
        FeatureClass::BaseCapability
    } else if matches!(
        feature,
        "gateway" | "marketplace" | "fs" | "deploy" | "acp_registry" | "lab-admin"
    ) {
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
    } else if matches!(feature, "upstream" | "test-utils") {
        FeatureClass::HelperInternal
    } else if lab_features.contains_key(feature) {
        FeatureClass::ServicePassthrough
    } else {
        FeatureClass::SdkOnly
    }
}

fn mapped_lab_feature(
    deps: &[String],
    api_features: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    deps.iter()
        .filter_map(|dep| dep.strip_prefix(LABBY_APIS_PREFIX))
        .find(|dep| api_features.contains_key(*dep))
        .map(|dep| format!("{LABBY_APIS_PREFIX}{dep}"))
}

fn exception_reason(classification: FeatureClass) -> Option<&'static str> {
    match classification {
        FeatureClass::ProductSlice => Some("standalone product slice"),
        FeatureClass::BaseCapability => Some("feature-gated base capability"),
        FeatureClass::BinaryOnly => Some("binary-only Lab feature"),
        FeatureClass::HelperInternal => Some("helper/internal feature"),
        FeatureClass::ExtractedCrate => Some("extracted crate feature"),
        FeatureClass::AggregateDefault => Some("aggregate/default feature"),
        FeatureClass::IntentionalException => Some("intentional crate-local exception"),
        FeatureClass::ServicePassthrough | FeatureClass::SdkOnly => None,
    }
}

fn service_feature(service: &str, matrix: &FeatureMatrix) -> Option<String> {
    let feature_name = match service {
        // Snippets are a built-in product service, but their execution/runtime
        // dependency is the `gateway` product slice rather than a same-named
        // Cargo feature. Keep generated service docs aligned with registration.
        "snippets" => "gateway",
        other => other,
    };

    matrix
        .features
        .iter()
        .find(|feature| {
            feature.crate_name == "labby"
                && feature.feature == feature_name
                && matches!(
                    feature.classification,
                    FeatureClass::ServicePassthrough
                        | FeatureClass::ProductSlice
                        | FeatureClass::BaseCapability
                        | FeatureClass::BinaryOnly
                )
        })
        .map(|feature| feature.feature.clone())
}

pub(super) fn service_surfaces(service: &str) -> SurfaceAvailability {
    SurfaceAvailability {
        cli: !matches!(service, "fs" | "stash"),
        mcp: true,
        api: service != "lab_admin",
        web_ui: matches!(
            service,
            "gateway"
                | "setup"
                | "fs"
                | "artifacts"
                | "sources"
                | "jobs"
                | "uploads"
                | "bundles"
                | "stash"
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
    Vec::new()
}

#[allow(clippy::too_many_lines)]
fn meta_for(name: &str) -> Option<&'static PluginMeta> {
    match name {
        "doctor" => Some(&crate::dispatch::doctor::META),
        "setup" => Some(&crate::dispatch::setup::META),
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
    fn stash_service_projection_matches_current_adapters_and_platform_boundary() {
        let projection = build_docs_projection(&workspace_root().unwrap()).unwrap();
        let stash = projection
            .service_catalog
            .iter()
            .find(|service| service.name == "stash")
            .expect("stash service inventory");

        assert!(!stash.surfaces.cli);
        assert!(stash.surfaces.mcp);
        assert!(stash.surfaces.api);
        assert!(stash.surfaces.web_ui);
        assert!(matches!(
            stash.exposure,
            ServiceExposure::RuntimeConditional
        ));
    }

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

    // The fs.preview action only exists in builds with the `fs` feature.
    #[cfg(feature = "fs")]
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

    #[test]
    fn proxy_projection_covers_cli_config_env_actions_and_service_inventory() {
        let projection = build_docs_projection(&workspace_root().unwrap()).unwrap();
        let service = projection
            .service_catalog
            .iter()
            .find(|service| service.name == "proxy")
            .expect("proxy service inventory");
        assert!(service.surfaces.cli);
        assert!(!service.surfaces.mcp);
        assert!(!service.surfaces.api);

        let config_keys = projection
            .proxy_config_reference
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            config_keys,
            BTreeSet::from([
                "auth",
                "bearer_token_env",
                "exposure",
                "inherit_env",
                "oauth_scopes",
                "path",
                "port",
                "port_range_end",
                "port_range_start",
                "shutdown_grace_ms",
            ])
        );
        assert!(projection.env_reference.iter().any(|entry| {
            entry.service == "proxy" && entry.env_var == "LABBY_PROXY_BEARER_TOKEN" && entry.secret
        }));

        for (service, action) in [
            ("setup", "proxy.configure"),
            ("doctor", "proxy.preflight"),
            ("gateway", "gateway.oauth.resource_lease.create"),
            ("gateway", "gateway.oauth.resource_lease.renew"),
            ("gateway", "gateway.oauth.resource_lease.release"),
        ] {
            assert!(
                projection
                    .action_catalog
                    .iter()
                    .any(|entry| entry.service == service && entry.action == action),
                "missing generated action {service}:{action}"
            );
        }

        let help = super::super::render::cli_help();
        for heading in [
            "## `labby proxy`",
            "## `labby setup proxy`",
            "## `labby doctor proxy`",
        ] {
            assert!(
                help.contains(heading),
                "missing generated CLI heading {heading}"
            );
        }

        assert!(
            projection
                .api_routes
                .iter()
                .any(|route| route.method == "POST" && route.path == "/v1/gateway")
        );
        #[cfg(feature = "api-docs")]
        for action in [
            "gateway.oauth.resource_lease.create",
            "gateway.oauth.resource_lease.renew",
            "gateway.oauth.resource_lease.release",
        ] {
            assert!(
                projection.openapi_json.contains(action),
                "OpenAPI omits {action}"
            );
        }
    }

    #[test]
    fn env_projection_covers_oauth_runtime_contract() {
        let projection = build_docs_projection(&workspace_root().unwrap()).unwrap();
        let vars = projection
            .env_reference
            .iter()
            .map(|entry| entry.env_var.as_str())
            .collect::<BTreeSet<_>>();
        for required in [
            "LABBY_AUTH_MODE",
            "LABBY_PUBLIC_URL",
            "LABBY_GOOGLE_CLIENT_ID",
            "LABBY_GOOGLE_CLIENT_SECRET",
            "LABBY_AUTH_ADMIN_EMAIL",
            "LABBY_AUTH_ALLOWED_REDIRECT_URIS",
            "LABBY_AUTH_ALLOWED_EMAIL_DOMAINS",
            "LABBY_AUTH_SQLITE_PATH",
            "LABBY_AUTH_KEY_PATH",
        ] {
            assert!(
                vars.contains(required),
                "missing OAuth env documentation for {required}"
            );
        }
        let projected = projection
            .env_reference
            .iter()
            .filter(|entry| entry.service == "auth")
            .map(|entry| entry.env_var.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            projected,
            BTreeSet::from([
                "LABBY_AUTH_ACCESS_TOKEN_TTL_SECS",
                "LABBY_AUTH_ADMIN_EMAIL",
                "LABBY_AUTH_ALLOWED_EMAIL_DOMAINS",
                "LABBY_AUTH_ALLOWED_REDIRECT_URIS",
                "LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE",
                "LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY",
                "LABBY_AUTH_CODE_TTL_SECS",
                "LABBY_AUTH_ENTERPRISE_ISSUERS_JSON",
                "LABBY_AUTH_KEY_PATH",
                "LABBY_AUTH_MACHINE_CLIENTS_JSON",
                "LABBY_AUTH_MAX_PENDING_OAUTH_STATES",
                "LABBY_AUTH_MODE",
                "LABBY_AUTH_PROVIDER",
                "LABBY_AUTH_REFRESH_TOKEN_TTL_SECS",
                "LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE",
                "LABBY_AUTH_SCOPES_SUPPORTED",
                "LABBY_AUTH_SQLITE_PATH",
                "LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE",
                "LABBY_AUTHELIA_CLIENT_ID",
                "LABBY_AUTHELIA_CLIENT_SECRET",
                "LABBY_AUTHELIA_CA_CERT_PATH",
                "LABBY_AUTHELIA_ISSUER_URL",
                "LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN",
                "LABBY_GOOGLE_CALLBACK_PATH",
                "LABBY_GOOGLE_CALLBACK_URL",
                "LABBY_GOOGLE_CLIENT_ID",
                "LABBY_GOOGLE_CLIENT_SECRET",
                "LABBY_GOOGLE_SCOPES",
                "LABBY_MCP_HTTP_TOKEN",
                "LABBY_PUBLIC_URL",
                "LABBY_TOKEN_ENCRYPTION_KEY",
            ])
        );
    }
}
