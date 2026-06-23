#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Integration test — `HttpClient::get_json` must inject the Auth header
//! and decode a JSON body into a user-provided type.

use serde::Deserialize;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

use labby_apis::core::{Auth, HttpClient};

#[derive(Debug, serde::Serialize, Deserialize, PartialEq)]
struct Pong {
    message: String,
}

#[tokio::test]
async fn get_json_injects_api_key_header_and_decodes_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .and(header("X-Api-Key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Pong {
            message: "pong".into(),
        }))
        .mount(&server)
        .await;

    let client = HttpClient::new(
        server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "secret".into(),
        },
    )
    .expect("HttpClient::new");

    let pong: Pong = client.get_json("/ping").await.expect("get_json");
    assert_eq!(
        pong,
        Pong {
            message: "pong".into()
        }
    );
}

#[tokio::test]
async fn get_json_returns_not_found_on_404() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let client = HttpClient::new(
        server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "secret".into(),
        },
    )
    .expect("HttpClient::new");

    let err: ApiError = client
        .get_json::<Pong>("/missing")
        .await
        .expect_err("should fail on 404");
    assert!(
        matches!(err, ApiError::NotFound),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn get_json_returns_auth_failed_on_401() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/secure"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = HttpClient::new(
        server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "wrong".into(),
        },
    )
    .expect("HttpClient::new");

    let err: ApiError = client
        .get_json::<Pong>("/secure")
        .await
        .expect_err("should fail on 401");
    assert!(matches!(err, ApiError::Auth), "expected Auth, got {err:?}");
}

#[tokio::test]
async fn get_json_returns_rate_limited_on_429() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/throttled"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let client = HttpClient::new(
        server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "secret".into(),
        },
    )
    .expect("HttpClient::new");

    let err: ApiError = client
        .get_json::<Pong>("/throttled")
        .await
        .expect_err("should fail on 429");
    assert!(
        matches!(err, ApiError::RateLimited { .. }),
        "expected RateLimited, got {err:?}"
    );
}

#[tokio::test]
async fn get_json_returns_server_error_on_500() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/boom"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
        .mount(&server)
        .await;

    let client = HttpClient::new(
        server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "secret".into(),
        },
    )
    .expect("HttpClient::new");

    let err: ApiError = client
        .get_json::<Pong>("/boom")
        .await
        .expect_err("should fail on 500");
    assert!(
        matches!(err, ApiError::Server { status: 500, .. }),
        "expected Server(500), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// post_graphql tests
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, PartialEq)]
struct GqlData {
    value: String,
}

#[tokio::test]
async fn post_graphql_decodes_data_on_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "value": "hello" }
        })))
        .mount(&server)
        .await;

    let client = HttpClient::new(server.uri(), Auth::None).expect("HttpClient::new");

    let data: GqlData = client
        .post_graphql("/graphql", "{ value }", None)
        .await
        .expect("post_graphql should succeed");
    assert_eq!(
        data,
        GqlData {
            value: "hello".into()
        }
    );
}

#[tokio::test]
async fn post_graphql_returns_server_error_on_errors_array() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "errors": [
                { "message": "field not found" },
                { "message": "permission denied" }
            ]
        })))
        .mount(&server)
        .await;

    let client = HttpClient::new(server.uri(), Auth::None).expect("HttpClient::new");

    let err: ApiError = client
        .post_graphql::<GqlData>("/graphql", "{ value }", None)
        .await
        .expect_err("should fail when errors[] present");

    match err {
        ApiError::Server {
            status: 200,
            ref body,
        } => {
            assert!(
                body.contains("field not found"),
                "body should contain first error: {body}"
            );
            assert!(
                body.contains("permission denied"),
                "body should contain second error: {body}"
            );
            assert!(
                body.contains("; "),
                "errors should be joined with '; ': {body}"
            );
        }
        other => {
            assert!(
                matches!(other, ApiError::Server { status: 200, .. }),
                "expected Server(200), got {other:?}"
            );
        }
    }
}

#[tokio::test]
async fn post_graphql_returns_error_when_both_data_and_errors_present() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "value": "partial" },
            "errors": [{ "message": "partial failure" }]
        })))
        .mount(&server)
        .await;

    let client = HttpClient::new(server.uri(), Auth::None).expect("HttpClient::new");

    let err: ApiError = client
        .post_graphql::<GqlData>("/graphql", "{ value }", None)
        .await
        .expect_err("errors should take priority over data");

    assert!(
        matches!(err, ApiError::Server { status: 200, .. }),
        "expected Server(200), got {err:?}"
    );
}

#[tokio::test]
async fn post_graphql_returns_decode_error_when_data_is_null() {
    use labby_apis::core::ApiError;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": null
        })))
        .mount(&server)
        .await;

    let client = HttpClient::new(server.uri(), Auth::None).expect("HttpClient::new");

    let err: ApiError = client
        .post_graphql::<GqlData>("/graphql", "{ value }", None)
        .await
        .expect_err("null data should yield decode error");

    match err {
        ApiError::Decode(ref msg) => {
            assert!(
                msg.contains("missing data field"),
                "message should mention missing data field: {msg}"
            );
        }
        other => {
            assert!(
                matches!(other, ApiError::Decode(_)),
                "expected Decode, got {other:?}"
            );
        }
    }
}
