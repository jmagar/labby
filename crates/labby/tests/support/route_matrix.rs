use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct RouteDescriptor {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) handler_group: String,
    pub(crate) handler_identity: String,
    pub(crate) feature: Option<String>,
    pub(crate) runtime_condition: Option<String>,
    pub(crate) auth_required: bool,
    pub(crate) bearer_only: bool,
    pub(crate) bootstrap_proof: bool,
    pub(crate) session_cookie_allowed: bool,
    pub(crate) csrf_required: bool,
    pub(crate) host_validation: bool,
    pub(crate) master_only: bool,
    pub(crate) cache_posture: String,
    pub(crate) failure_disclosure: String,
    pub(crate) side_effects: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RequestClass {
    Public,
    Bearer,
    BrowserSession,
    BootstrapProof,
    OAuthProtocol,
    RelayAdmin,
    HostValidated,
    Development,
    StaticAsset,
    Mcp,
    ServiceDispatch,
}

#[derive(Clone, Debug)]
pub(crate) struct RouteCase {
    pub(crate) descriptor: RouteDescriptor,
    pub(crate) path: String,
    pub(crate) class: RequestClass,
    pub(crate) body: Option<&'static str>,
}

impl RouteCase {
    pub(crate) fn key(&self) -> String {
        format!("{} {}", self.descriptor.method, self.descriptor.path)
    }

    pub(crate) fn permits_runtime_absence(&self) -> bool {
        self.descriptor.runtime_condition.is_some()
            || self
                .descriptor
                .feature
                .as_deref()
                .is_some_and(|feature| !feature_is_compiled(feature))
    }

    pub(crate) fn is_compiled(&self) -> bool {
        self.descriptor
            .feature
            .as_deref()
            .is_none_or(feature_is_compiled)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SecurityInvariant {
    pub(crate) name: &'static str,
    pub(crate) class: RequestClass,
}

pub(crate) const SECURITY_INVARIANTS: &[SecurityInvariant] = &[
    SecurityInvariant {
        name: "public",
        class: RequestClass::Public,
    },
    SecurityInvariant {
        name: "bearer",
        class: RequestClass::Bearer,
    },
    SecurityInvariant {
        name: "browser-session",
        class: RequestClass::BrowserSession,
    },
    SecurityInvariant {
        name: "bootstrap-proof",
        class: RequestClass::BootstrapProof,
    },
    SecurityInvariant {
        name: "relay-admin",
        class: RequestClass::RelayAdmin,
    },
    SecurityInvariant {
        name: "mcp",
        class: RequestClass::Mcp,
    },
    SecurityInvariant {
        name: "oauth-protocol",
        class: RequestClass::OAuthProtocol,
    },
    SecurityInvariant {
        name: "host-validated",
        class: RequestClass::HostValidated,
    },
    SecurityInvariant {
        name: "development",
        class: RequestClass::Development,
    },
    SecurityInvariant {
        name: "static-asset",
        class: RequestClass::StaticAsset,
    },
    SecurityInvariant {
        name: "service-dispatch",
        class: RequestClass::ServiceDispatch,
    },
];

pub(crate) const PINNED_ROUTE_COUNT: usize = 120;
pub(crate) const PINNED_METHOD_PATH_SHA256: &str =
    "4861d6e4bf1d6c9858481604852ad2f29c8da66eff7db33c54ef31c2fc48bd6a";

impl SecurityInvariant {
    pub(crate) fn validate_descriptor(&self, route: &RouteDescriptor) -> Result<(), String> {
        let mutation = matches!(route.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
        let expected = match self.class {
            RequestClass::Public | RequestClass::StaticAsset => {
                (false, false, false, false, false, false)
            }
            RequestClass::Bearer => (true, false, false, false, false, false),
            RequestClass::BrowserSession => {
                let host = matches!(
                    route.handler_group.as_str(),
                    "doctor" | "setup" | "bundles" | "jobs" | "sources" | "uploads"
                ) || route.path == "/auth/local-session";
                (true, false, false, true, mutation, host)
            }
            RequestClass::BootstrapProof => (true, false, true, false, false, true),
            RequestClass::OAuthProtocol => {
                let protected = route.auth_required;
                (
                    protected,
                    false,
                    false,
                    protected,
                    protected && mutation,
                    false,
                )
            }
            RequestClass::RelayAdmin => (true, false, false, true, mutation, false),
            RequestClass::HostValidated => (true, true, false, false, false, true),
            RequestClass::Development => (true, false, false, true, false, false),
            RequestClass::Mcp => (true, true, false, false, false, false),
            RequestClass::ServiceDispatch => (true, false, false, true, false, false),
        };
        let actual = (
            route.auth_required,
            route.bearer_only,
            route.bootstrap_proof,
            route.session_cookie_allowed,
            route.csrf_required,
            route.host_validation,
        );
        if actual != expected {
            return Err(format!(
                "{} security axes mismatch for {} {}: expected {expected:?}, got {actual:?}",
                self.name, route.method, route.path
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_invalid_outcome(
        &self,
        route: &RouteDescriptor,
        status: reqwest::StatusCode,
    ) -> Result<(), String> {
        // OAuth session discovery and idempotent logout deliberately expose an
        // anonymous protocol result even though a valid session enriches it.
        if self.class == RequestClass::OAuthProtocol {
            return if status.is_server_error() {
                Err(format!(
                    "{} {} returned protocol server failure {status}",
                    route.method, route.path
                ))
            } else {
                Ok(())
            };
        }
        if route.auth_required
            && !matches!(
                status,
                reqwest::StatusCode::UNAUTHORIZED
                    | reqwest::StatusCode::FORBIDDEN
                    | reqwest::StatusCode::NOT_FOUND
            )
        {
            return Err(format!(
                "{} {} accepted invalid credentials with {status}",
                route.method, route.path
            ));
        }
        Ok(())
    }
}

pub(crate) fn invariant_for(class: RequestClass) -> &'static SecurityInvariant {
    SECURITY_INVARIANTS
        .iter()
        .find(|invariant| invariant.class == class)
        .expect("every request class has a security invariant")
}

pub(crate) fn route_cases() -> Result<Vec<RouteCase>, String> {
    let descriptors: Vec<RouteDescriptor> =
        serde_json::from_str(include_str!("../../../../docs/generated/api-routes.json"))
            .map_err(|error| format!("parse generated route inventory: {error}"))?;
    let mut seen = BTreeSet::new();
    let mut cases = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        let key = format!("{} {}", descriptor.method, descriptor.path);
        if !seen.insert(key.clone()) {
            return Err(format!("duplicate route recipe: {key}"));
        }
        cases.push(RouteCase {
            path: sample_path(&descriptor.path)?,
            class: classify(&descriptor),
            body: request_body(&descriptor),
            descriptor,
        });
    }
    Ok(cases)
}

fn classify(route: &RouteDescriptor) -> RequestClass {
    if route.bootstrap_proof {
        RequestClass::BootstrapProof
    } else if route.handler_group == "mcp" || route.handler_group == "protected_mcp" {
        RequestClass::Mcp
    } else if route.handler_group == "oauth_relay" && route.auth_required {
        RequestClass::RelayAdmin
    } else if route.handler_group == "oauth" {
        RequestClass::OAuthProtocol
    } else if route.handler_group == "dev" {
        RequestClass::Development
    } else if route.handler_group == "apps" && route.path.contains("assets") {
        RequestClass::StaticAsset
    } else if route.handler_group == "services" {
        RequestClass::ServiceDispatch
    } else if route.session_cookie_allowed {
        RequestClass::BrowserSession
    } else if route.host_validation {
        RequestClass::HostValidated
    } else if route.auth_required {
        RequestClass::Bearer
    } else {
        RequestClass::Public
    }
}

fn feature_is_compiled(feature: &str) -> bool {
    (feature == "gateway" && cfg!(feature = "gateway"))
        || (feature == "fs" && cfg!(feature = "fs"))
        || (feature == "skills" && cfg!(feature = "skills"))
        || (feature == "lab-admin" && cfg!(feature = "lab-admin"))
        || (feature == "api-docs" && cfg!(feature = "api-docs"))
        || (feature == "systemd" && cfg!(feature = "systemd"))
}

fn request_body(route: &RouteDescriptor) -> Option<&'static str> {
    match (route.method.as_str(), route.handler_group.as_str()) {
        ("POST", "mcp") => Some(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"route-matrix","version":"1"}}}"#,
        ),
        ("POST" | "PUT", _) => Some("{}"),
        _ => None,
    }
}

fn sample_path(template: &str) -> Result<String, String> {
    let substitutions = BTreeMap::from([
        ("{machine_id}", "missing-machine"),
        ("{credential_id}", "missing-credential"),
        ("{email}", "nobody%40example.invalid"),
        ("{interaction}", "missing-interaction"),
        ("{operation_id}", "missing-operation"),
        ("{provider_id}", "missing-provider"),
        ("{file_id}", "01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        ("{grant_id}", "01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        ("{id}", "missing-upload"),
        ("{name}", "missing"),
        ("{service}", "doctor"),
        ("{*route}", "missing"),
        ("{*rest}", "missing"),
        ("{*suffix}", "missing"),
        ("{runtime_protected_mcp_route}", "operator"),
    ]);
    let mut path = template.to_owned();
    for (parameter, sample) in substitutions {
        path = path.replace(parameter, sample);
    }
    if path.contains('{') || path.contains('}') {
        return Err(format!(
            "route template lacks a sample substitution: {template}"
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    #[test]
    fn every_inventory_row_has_exactly_one_complete_recipe() {
        let cases = route_cases().expect("route cases");
        assert!(!cases.is_empty());
        assert_eq!(
            cases
                .iter()
                .map(RouteCase::key)
                .collect::<BTreeSet<_>>()
                .len(),
            cases.len()
        );
        assert!(
            cases
                .iter()
                .all(|case| case.path.starts_with('/') && !case.path.contains('{'))
        );
        let pinned_material = cases
            .iter()
            .map(RouteCase::key)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(cases.len(), PINNED_ROUTE_COUNT);
        assert_eq!(
            hex::encode(Sha256::digest(pinned_material.as_bytes())),
            PINNED_METHOD_PATH_SHA256,
            "the independent method/path denominator changed; review and deliberately repin"
        );
    }

    #[test]
    fn security_oracle_covers_each_protected_request_class() {
        let cases = route_cases().expect("route cases");
        for class in cases
            .iter()
            .filter(|case| case.descriptor.auth_required)
            .map(|case| case.class)
            .collect::<BTreeSet<_>>()
        {
            assert!(
                SECURITY_INVARIANTS
                    .iter()
                    .any(|invariant| invariant.class == class),
                "missing security invariant for {class:?}"
            );
        }
        for case in &cases {
            let result = invariant_for(case.class).validate_descriptor(&case.descriptor);
            assert!(result.is_ok(), "{}: {result:?}", case.key());
        }
    }

    #[test]
    fn every_security_axis_is_kill_sensitive_across_route_classes() {
        let cases = route_cases().expect("route cases");
        for class in SECURITY_INVARIANTS.iter().map(|invariant| invariant.class) {
            let Some(case) = cases.iter().find(|case| case.class == class) else {
                continue;
            };
            for axis in 0..6 {
                let mut killed = case.descriptor.clone();
                match axis {
                    0 => killed.auth_required = !killed.auth_required,
                    1 => killed.bearer_only = !killed.bearer_only,
                    2 => killed.bootstrap_proof = !killed.bootstrap_proof,
                    3 => killed.session_cookie_allowed = !killed.session_cookie_allowed,
                    4 => killed.csrf_required = !killed.csrf_required,
                    5 => killed.host_validation = !killed.host_validation,
                    _ => unreachable!(),
                }
                assert!(
                    invariant_for(class).validate_descriptor(&killed).is_err(),
                    "{class:?} axis {axis} mutation survived for {}",
                    case.key()
                );
            }
        }
    }
}
