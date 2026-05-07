#![cfg(feature = "beads")]

use lab_apis::beads::{BeadsClient, DoltConnection};

fn make_client() -> BeadsClient {
    BeadsClient::new(DoltConnection {
        url: "mysql://127.0.0.1:3306/".to_string(),
        user: Some("root".to_string()),
        password: None,
        default_project: Some("lab".to_string()),
    })
    .expect("dolt connection options should parse")
}

#[test]
fn beads_contract_is_read_only() {
    let client = make_client();
    let contract = client.contract_status();

    assert_eq!(contract.status, "dolt_sql_implemented");
    assert!(contract.safe_v1_actions.contains(&"issue.ready"));
    assert!(contract.safe_v1_actions.contains(&"graph.show"));
    assert!(contract.safe_v1_actions.contains(&"project.list"));
    assert!(contract.deferred.contains(&"issue.create"));
    assert!(contract.deferred.contains(&"dolt.push"));
}

#[test]
fn default_project_is_exposed() {
    let client = make_client();
    assert_eq!(client.default_project(), Some("lab"));
}

#[test]
fn resolve_project_uses_default_when_unspecified() {
    let client = make_client();
    let project = client.resolve_project(None).expect("default project");
    assert_eq!(project, "lab");
}

#[test]
fn resolve_project_rejects_invalid_identifiers() {
    let client = make_client();
    assert!(client.resolve_project(Some("good-1")).is_ok());
    assert!(client.resolve_project(Some("bad name")).is_err());
    assert!(client.resolve_project(Some("`evil")).is_err());
}
