#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/state_snapshot.rs"]
mod state_snapshot;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use reqwest::StatusCode;
#[cfg(target_os = "linux")]
use serde_json::Value;
use state_snapshot::{NarrowStorageObservation, OwnedProcessObservation, PERSISTENCE_CONTRACT};

#[test]
fn restart_suite_locks_the_complete_persistence_contract() {
    assert_eq!(PERSISTENCE_CONTRACT.len(), 11);
}

#[tokio::test]
async fn cold_start_and_repeated_restart_preserve_durable_identity_and_replace_process_state() {
    let mut identity = live_identity::LiveIdentity::bootstrap("parity-restart-subject")
        .await
        .expect("cold public bootstrap");
    let root = identity.root().to_path_buf();
    let original_base = identity.base().to_owned();
    let durable_before =
        NarrowStorageObservation::read(&root.join("labby-home"), &["config.toml", "access.sqlite"])
            .unwrap();
    let first = OwnedProcessObservation::read(&root).unwrap();
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );

    identity.restart().await.expect("first staged restart");
    let second = OwnedProcessObservation::read(&root).unwrap();
    second.assert_restarted_from(&first);
    assert_eq!(identity.base(), original_base);
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(
        NarrowStorageObservation::read(&root.join("labby-home"), &["config.toml", "access.sqlite"])
            .unwrap(),
        durable_before
    );

    identity.restart().await.expect("repeated restart");
    let third = OwnedProcessObservation::read(&root).unwrap();
    third.assert_restarted_from(&second);
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK,
        "allowed upstream disappeared after repeated restart"
    );

    let cleanup = identity.cleanup().await.expect("journaled cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
    assert!(!root.exists(), "owned installation survived cleanup");
}

#[tokio::test]
async fn staged_protected_route_change_is_not_half_published_before_restart() {
    let mut identity = live_identity::LiveIdentity::bootstrap("staged-route-subject")
        .await
        .expect("bootstrap");
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );
    let before = OwnedProcessObservation::read(identity.root()).unwrap();
    let disabled =
        live_identity::policy(&["lab:read"]).replace("enabled = true", "enabled = false");
    std::fs::write(identity.root().join("labby-home/config.toml"), disabled).unwrap();

    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK,
        "desired config leaked into the running route collection"
    );
    assert_eq!(
        OwnedProcessObservation::read(identity.root()).unwrap(),
        before
    );

    identity.restart().await.expect("activate staged revision");
    OwnedProcessObservation::read(identity.root())
        .unwrap()
        .assert_restarted_from(&before);
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::NOT_FOUND,
        "disabled desired route remained mounted after restart"
    );
    let cleanup = identity.cleanup().await.expect("cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn file_stash_round_trips_across_two_principals_and_restart() {
    use base64::Engine as _;
    use sha2::{Digest as _, Sha256};

    let mut identity = live_identity::LiveIdentity::bootstrap_with_scopes(
        "stash-live-owner",
        &["lab:read", "lab:admin"],
    )
    .await
    .expect("bootstrap owner");
    identity
        .provision_stash_recipient("stash-live-recipient", "Stash recipient")
        .await
        .expect("provision recipient through live identity fixture");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let bytes = b"real daemon stash bytes\n";
    let upload: Value = client
        .post(format!("{}/v1/stash/uploads", identity.base()))
        .bearer_auth(identity.credential_for_request())
        .header("x-labby-stash-filename", "journey.txt")
        .body(bytes.as_slice())
        .send()
        .await
        .expect("upload request")
        .error_for_status()
        .expect("upload status")
        .json()
        .await
        .expect("upload response");
    let file_id = upload["file_id"].as_str().expect("file id").to_owned();
    assert_eq!(upload["uri"], format!("stash://me/files/{file_id}"));

    let owner_list: Value = client
        .get(format!("{}/v1/stash", identity.base()))
        .bearer_auth(identity.credential_for_request())
        .send()
        .await
        .expect("owner list")
        .error_for_status()
        .expect("owner list status")
        .json()
        .await
        .expect("owner list body");
    assert_eq!(owner_list["files"][0]["file_id"], file_id);
    assert_eq!(owner_list["files"][0]["owned"], true);

    let owner_search: Value = client
        .get(format!("{}/v1/stash?query=JOURNEY", identity.base()))
        .bearer_auth(identity.credential_for_request())
        .send()
        .await
        .expect("owner search")
        .error_for_status()
        .expect("owner search status")
        .json()
        .await
        .expect("owner search body");
    assert_eq!(owner_search["files"][0]["file_id"], file_id);

    let owner_metadata: Value = client
        .get(format!("{}/v1/stash/files/{file_id}", identity.base()))
        .bearer_auth(identity.credential_for_request())
        .send()
        .await
        .expect("owner metadata")
        .error_for_status()
        .expect("owner metadata status")
        .json()
        .await
        .expect("owner metadata body");
    assert_eq!(owner_metadata["file_id"], file_id);
    assert_eq!(owner_metadata["owned"], true);

    let grant: Value = client
        .post(format!(
            "{}/v1/stash/files/{file_id}/grants",
            identity.base()
        ))
        .bearer_auth(identity.credential_for_request())
        .json(&serde_json::json!({"grantee_principal_id":"stash-live-recipient"}))
        .send()
        .await
        .expect("grant request")
        .error_for_status()
        .expect("grant status")
        .json()
        .await
        .expect("grant response");
    let grant_id = grant["grant_id"].as_str().expect("grant id").to_owned();

    let grants: Value = client
        .get(format!(
            "{}/v1/stash/files/{file_id}/grants",
            identity.base()
        ))
        .bearer_auth(identity.credential_for_request())
        .send()
        .await
        .expect("grant list")
        .error_for_status()
        .expect("grant list status")
        .json()
        .await
        .expect("grant list body");
    assert_eq!(grants["grants"][0]["grant_id"], grant_id);
    assert_eq!(
        grants["grants"][0]["grantee_principal_id"],
        "stash-live-recipient"
    );

    let owner_stats: Value = client
        .get(format!("{}/v1/stash/stats", identity.base()))
        .bearer_auth(identity.credential_for_request())
        .send()
        .await
        .expect("owner stats")
        .error_for_status()
        .expect("owner stats status")
        .json()
        .await
        .expect("owner stats body");
    assert_eq!(owner_stats["owned_file_count"], 1);
    assert_eq!(owner_stats["owned_shared_file_count"], 1);
    assert_eq!(owner_stats["owned_committed_bytes"], bytes.len());

    let mcp_headers = |request: reqwest::RequestBuilder| {
        request
            .bearer_auth(identity.credential_for_request())
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
    };
    let initialized = mcp_headers(client.post(format!("{}/mcp", identity.base())))
        .json(&serde_json::json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"stash-live","version":"1"}}
        }))
        .send()
        .await
        .expect("MCP initialize");
    assert_eq!(initialized.status(), StatusCode::OK);
    let mcp_read: Value = mcp_headers(client.post(format!("{}/mcp", identity.base())))
        .json(&serde_json::json!({
            "jsonrpc":"2.0", "id":2, "method":"resources/read",
            "params":{"uri":format!("stash://me/files/{file_id}")}
        }))
        .send()
        .await
        .expect("MCP resource read")
        .error_for_status()
        .expect("MCP resource status")
        .json()
        .await
        .expect("MCP resource response");
    let encoded = mcp_read["result"]["contents"][0]["blob"]
        .as_str()
        .expect("MCP blob");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("MCP base64");
    assert_eq!(Sha256::digest(&decoded), Sha256::digest(bytes));

    let recipient_download = client
        .get(format!(
            "{}/v1/stash/files/{file_id}/content",
            identity.base()
        ))
        .bearer_auth(identity.static_token_for_request())
        .send()
        .await
        .expect("recipient download")
        .error_for_status()
        .expect("recipient authorization")
        .bytes()
        .await
        .expect("download bytes");
    assert_eq!(Sha256::digest(&recipient_download), Sha256::digest(bytes));

    let recipient_list: Value = client
        .get(format!("{}/v1/stash", identity.base()))
        .bearer_auth(identity.static_token_for_request())
        .send()
        .await
        .expect("recipient list")
        .error_for_status()
        .expect("recipient list status")
        .json()
        .await
        .expect("recipient list body");
    assert_eq!(recipient_list["files"][0]["file_id"], file_id);
    assert_eq!(recipient_list["files"][0]["owned"], false);
    let recipient_stats: Value = client
        .get(format!("{}/v1/stash/stats", identity.base()))
        .bearer_auth(identity.static_token_for_request())
        .send()
        .await
        .expect("recipient stats")
        .error_for_status()
        .expect("recipient stats status")
        .json()
        .await
        .expect("recipient stats body");
    assert_eq!(recipient_stats["owned_file_count"], 0);
    assert_eq!(recipient_stats["owned_committed_bytes"], 0);
    assert_eq!(
        client
            .delete(format!("{}/v1/stash/files/{file_id}", identity.base()))
            .bearer_auth(identity.static_token_for_request())
            .send()
            .await
            .expect("recipient owner-only delete")
            .status(),
        StatusCode::NOT_FOUND
    );

    identity
        .restart()
        .await
        .expect("restart with persisted stash");
    let persisted = client
        .get(format!(
            "{}/v1/stash/files/{file_id}/content",
            identity.base()
        ))
        .bearer_auth(identity.static_token_for_request())
        .send()
        .await
        .expect("post-restart download")
        .error_for_status()
        .expect("post-restart authorization")
        .bytes()
        .await
        .expect("post-restart bytes");
    assert_eq!(Sha256::digest(&persisted), Sha256::digest(bytes));

    assert_eq!(
        client
            .delete(format!(
                "{}/v1/stash/files/{file_id}/grants/{grant_id}",
                identity.base()
            ))
            .bearer_auth(identity.credential_for_request())
            .send()
            .await
            .expect("revoke")
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        client
            .get(format!(
                "{}/v1/stash/files/{file_id}/content",
                identity.base()
            ))
            .bearer_auth(identity.static_token_for_request())
            .send()
            .await
            .expect("revoked read")
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .delete(format!("{}/v1/stash/files/{file_id}", identity.base()))
            .bearer_auth(identity.credential_for_request())
            .send()
            .await
            .expect("delete")
            .status(),
        StatusCode::NO_CONTENT
    );
    let stats: Value = client
        .get(format!("{}/v1/stash/stats", identity.base()))
        .bearer_auth(identity.credential_for_request())
        .send()
        .await
        .expect("stats")
        .error_for_status()
        .expect("stats status")
        .json()
        .await
        .expect("stats body");
    assert_eq!(stats["owned_file_count"], 0);
    assert_eq!(stats["owned_committed_bytes"], 0);

    let root = identity.root().to_path_buf();
    let cleanup = identity.cleanup().await.expect("cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
    assert!(!root.exists(), "owned installation survived cleanup");
}
