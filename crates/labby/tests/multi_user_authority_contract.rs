use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

const MATRIX: &str = include_str!("../../../docs/access-control/authority-matrix-v1.json");
const ACTIONS: &str = include_str!("../../../docs/generated/action-catalog.json");

#[test]
fn every_registered_action_has_exactly_one_authority_classification() {
    let matrix: Value = serde_json::from_str(MATRIX).expect("valid authority matrix");
    let actions: Value = serde_json::from_str(ACTIONS).expect("valid generated action catalog");

    let classifications = matrix["serviceClassifications"]
        .as_array()
        .expect("service classifications");
    let mut by_service = BTreeMap::new();
    for entry in classifications {
        let service = entry["service"].as_str().expect("classified service");
        assert!(
            by_service.insert(service, entry).is_none(),
            "duplicate authority classification for {service}"
        );
    }

    for action in actions.as_array().expect("action catalog array") {
        let service = action["service"].as_str().expect("action service");
        let name = action["action"].as_str().expect("action name");
        let classification = by_service
            .get(service)
            .expect("every registered service must have an authority classification");
        for field in [
            "resourceFamily",
            "readCapability",
            "operateCapability",
            "manageCapability",
        ] {
            assert!(
                classification[field]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "{service}:{name} has no {field}"
            );
        }
    }
}

#[test]
fn matrix_roles_and_resources_are_closed_over_registered_vocabulary() {
    let matrix: Value = serde_json::from_str(MATRIX).expect("valid authority matrix");
    assert_eq!(matrix["schemaVersion"], "labby.authority-matrix/v1");

    let owner_scopes = strings(&matrix["ownerScopes"]);
    assert_eq!(
        owner_scopes,
        BTreeSet::from(["installation", "personal", "project", "team"])
    );

    let roles = matrix["roles"].as_object().expect("role templates");
    for required in [
        "platform_admin",
        "team_owner",
        "team_admin",
        "team_member",
        "personal_user",
        "project_owner",
        "project_admin",
        "project_member",
        "project_viewer",
    ] {
        assert!(
            roles.contains_key(required),
            "missing role template {required}"
        );
    }

    let capabilities: BTreeSet<&str> = roles.values().flat_map(strings).collect();
    let resources = matrix["resourceFamilies"]
        .as_object()
        .expect("resource families");
    for (name, resource) in resources {
        let scopes = strings(&resource["ownerScopes"]);
        assert!(!scopes.is_empty(), "resource {name} has no owner scope");
        assert!(
            scopes.is_subset(&owner_scopes),
            "resource {name} has an unknown owner scope"
        );
    }

    for entry in matrix["serviceClassifications"]
        .as_array()
        .expect("service classifications")
    {
        let family = entry["resourceFamily"].as_str().expect("resource family");
        assert!(resources.contains_key(family), "unknown family {family}");
        for field in ["readCapability", "operateCapability", "manageCapability"] {
            let capability = entry[field].as_str().expect("capability");
            assert!(
                capabilities.contains(capability),
                "unregistered capability {capability}"
            );
        }
    }
}

fn strings(value: &Value) -> BTreeSet<&str> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|item| item.as_str().expect("string entry"))
        .collect()
}
