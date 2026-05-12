#![allow(clippy::expect_used)]

use lab_apis::bytestash::ByteStashClient;
use lab_apis::core::Auth;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn make_client(base_url: &str) -> ByteStashClient {
    ByteStashClient::new(base_url, Auth::None).expect("client construction")
}

#[tokio::test]
async fn snippet_get_encodes_dynamic_path_segment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/snippets/folder%2Fitem%3Fv=1%23frag"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "folder/item?v=1#frag"
        })))
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let result = client
        .snippet_get("folder/item?v=1#frag")
        .await
        .expect("snippet_get");
    assert_eq!(result["id"], "folder/item?v=1#frag");
}

#[tokio::test]
async fn share_get_encodes_dynamic_path_segment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/share/share%2Fid%3Fx=1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "share": true
        })))
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let result = client
        .snippets_share_get("share/id?x=1")
        .await
        .expect("share_get");
    assert_eq!(result["share"], true);
}
