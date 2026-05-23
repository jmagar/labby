//! Qdrant HTTP client — collection management, upsert, hybrid search.

use std::sync::LazyLock;
use std::time::Duration;

use super::error::QdrantError;
use super::types::{QueryResponse, SearchHit, SparseVector, UpsertPoint};
use crate::core::Auth;
use reqwest::RequestBuilder;
use serde::Serialize;
use serde_json::json;

/// Shared HTTP client. `reqwest::Client` owns a connection pool; constructing one
/// per call exhausts sockets and bypasses pooling.
///
/// Builder failure (rustls/TLS init only) falls back to `reqwest::Client::new()`
/// — losing the tuned pool/timeout settings but preserving forward progress.
/// Library code MUST NOT panic at init.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

pub struct QdrantClient {
    base_url: String,
    auth: Auth,
}

impl QdrantClient {
    pub fn new(base_url: &str) -> Self {
        Self::with_auth(base_url, Auth::None)
    }

    pub fn with_auth(base_url: &str, auth: Auth) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
        }
    }

    fn collection_url(&self, collection: &str) -> String {
        format!("{}/collections/{collection}", self.base_url)
    }

    fn points_url(&self, collection: &str, path: &str) -> String {
        if path.is_empty() {
            format!("{}/collections/{collection}/points", self.base_url)
        } else {
            format!("{}/collections/{collection}/points/{path}", self.base_url)
        }
    }

    fn apply_auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.auth {
            Auth::None => req,
            Auth::ApiKey { header, key } => req.header(header, key),
            Auth::Token { token } => req.header("Authorization", format!("Token {token}")),
            Auth::Bearer { token } => req.bearer_auth(token),
            Auth::Basic { username, password } => req.basic_auth(username, Some(password)),
            Auth::Session { cookie } => req.header("Cookie", cookie),
        }
    }

    /// Create the collection if it does not exist; no-op if it already exists.
    ///
    /// Schema: named `dense` (Cosine, `dense_dim`-dimensional) + named `bm42` sparse
    /// with server-side IDF. Matches the schema used by the `axon` collection.
    pub async fn ensure_named_collection(
        &self,
        collection: &str,
        dense_dim: usize,
    ) -> Result<(), QdrantError> {
        // Check if collection already exists.
        let check_url = self.collection_url(collection);
        let resp = self.apply_auth(HTTP_CLIENT.get(&check_url)).send().await?;
        if resp.status().is_success() {
            return Ok(());
        }

        let create_url = check_url;
        let body = json!({
            "vectors": {
                "dense": {
                    "size": dense_dim,
                    "distance": "Cosine",
                    "on_disk": true
                }
            },
            "sparse_vectors": {
                "bm42": {
                    "modifier": "idf"
                }
            },
            "on_disk_payload": true
        });
        let resp = self
            .apply_auth(HTTP_CLIENT.put(&create_url).json(&body))
            .send()
            .await?;
        // 409 = already exists (race-safe).
        if !resp.status().is_success() && resp.status().as_u16() != 409 {
            let status = resp.status().as_u16();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(QdrantError::Api {
                status,
                body: body_text,
            });
        }
        Ok(())
    }

    /// Upsert a batch of points with named dense + sparse vectors.
    pub async fn upsert_points(
        &self,
        collection: &str,
        points: &[UpsertPoint],
    ) -> Result<(), QdrantError> {
        if points.is_empty() {
            return Ok(());
        }

        #[derive(Serialize)]
        struct VectorMap<'a> {
            dense: &'a [f32],
            bm42: &'a SparseVector,
        }

        #[derive(Serialize)]
        struct PointBody<'a> {
            id: u64,
            vector: VectorMap<'a>,
            payload: &'a serde_json::Value,
        }

        let bodies: Vec<PointBody<'_>> = points
            .iter()
            .map(|p| PointBody {
                id: p.id,
                vector: VectorMap {
                    dense: &p.dense,
                    bm42: &p.sparse,
                },
                payload: &p.payload,
            })
            .collect();

        // Qdrant upsert: PUT /collections/{name}/points?wait=true
        let url = format!("{}?wait=true", self.points_url(collection, ""));
        let resp = self
            .apply_auth(HTTP_CLIENT.put(&url).json(&json!({ "points": bodies })))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(QdrantError::Api { status, body });
        }
        Ok(())
    }

    /// Hybrid search: dense + BM42 sparse prefetch with RRF fusion.
    ///
    /// Uses Qdrant's `/points/query` endpoint with two prefetch arms. Requires a Named
    /// collection (dense + bm42). `candidates` is per-arm prefetch size (≥ limit).
    pub async fn hybrid_search(
        &self,
        collection: &str,
        dense: &[f32],
        sparse: &SparseVector,
        limit: usize,
        candidates: usize,
    ) -> Result<Vec<SearchHit>, QdrantError> {
        let url = self.points_url(collection, "query");
        let body = json!({
            "prefetch": [
                {
                    "query": dense,
                    "using": "dense",
                    "limit": candidates,
                    "params": {
                        "hnsw_ef": 128,
                        "quantization": { "rescore": true, "oversampling": 1.5 }
                    }
                },
                {
                    "query": sparse,
                    "using": "bm42",
                    "limit": candidates
                }
            ],
            "query": { "fusion": "rrf" },
            "limit": limit,
            "with_payload": true,
            "with_vector": false
        });
        let resp = self
            .apply_auth(HTTP_CLIENT.post(&url).json(&body))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(QdrantError::Api {
                status,
                body: body_text,
            });
        }
        let parsed: QueryResponse = resp.json().await?;
        Ok(parsed.result.points)
    }

    /// Dense-only search for Named collections (fallback when sparse vector is empty).
    pub async fn dense_search(
        &self,
        collection: &str,
        dense: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchHit>, QdrantError> {
        let url = self.points_url(collection, "query");
        let body = json!({
            "query": dense,
            "using": "dense",
            "limit": limit,
            "with_payload": true,
            "with_vector": false,
            "params": {
                "hnsw_ef": 128,
                "quantization": { "rescore": true, "oversampling": 1.5 }
            }
        });
        let resp = self
            .apply_auth(HTTP_CLIENT.post(&url).json(&body))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(QdrantError::Api {
                status,
                body: body_text,
            });
        }
        let parsed: QueryResponse = resp.json().await?;
        Ok(parsed.result.points)
    }

    /// Delete all points with a given payload field value.
    pub async fn delete_by_payload(
        &self,
        collection: &str,
        field: &str,
        value: &str,
    ) -> Result<(), QdrantError> {
        // Qdrant delete by filter: POST /collections/{name}/points/delete
        let url = format!("{}?wait=true", self.points_url(collection, "delete"));
        let body = json!({
            "filter": {
                "must": [{ "key": field, "match": { "value": value } }]
            }
        });
        let resp = self
            .apply_auth(HTTP_CLIENT.post(&url).json(&body))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(QdrantError::Api {
                status,
                body: body_text,
            });
        }
        Ok(())
    }

    /// Delete points matching an arbitrary Qdrant filter JSON.
    ///
    /// Generic primitive; callers build the filter shape. Used by the
    /// tool-search indexer to compose an upsert-then-sweep pattern via
    /// `must: field==value AND must_not: has_id in keep_ids`.
    pub async fn delete_by_filter(
        &self,
        collection: &str,
        filter: serde_json::Value,
    ) -> Result<(), QdrantError> {
        let url = format!("{}?wait=true", self.points_url(collection, "delete"));
        let resp = self
            .apply_auth(HTTP_CLIENT.post(&url).json(&json!({ "filter": filter })))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(QdrantError::Api {
                status,
                body: body_text,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Auth;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const COLLECTION: &str = "lab-tools";
    const QDRANT_API_KEY: &str = "qdrant-test-key";

    fn authed_client(base_url: &str) -> QdrantClient {
        QdrantClient::with_auth(
            base_url,
            Auth::ApiKey {
                header: "api-key".to_string(),
                key: QDRANT_API_KEY.to_string(),
            },
        )
    }

    fn point() -> UpsertPoint {
        UpsertPoint {
            id: 42,
            dense: vec![0.1, 0.2],
            sparse: SparseVector {
                indices: vec![7],
                values: vec![1.0],
            },
            payload: json!({ "name": "demo" }),
        }
    }

    #[tokio::test]
    async fn test_hybrid_search_returns_parsed_hits() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/lab-tools/points/query"))
            .and(header("api-key", QDRANT_API_KEY))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result": {
                    "points": [{
                        "id": 42,
                        "score": 0.91,
                        "payload": { "name": "demo", "upstream": "fixture" }
                    }]
                }
            })))
            .mount(&server)
            .await;

        let hits = authed_client(&server.uri())
            .hybrid_search(
                COLLECTION,
                &[0.1, 0.2],
                &SparseVector {
                    indices: vec![7],
                    values: vec![1.0],
                },
                5,
                10,
            )
            .await
            .expect("hybrid search should parse");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 0.91);
        assert_eq!(hits[0].payload["name"], "demo");
    }

    #[tokio::test]
    async fn test_hybrid_search_propagates_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/lab-tools/points/query"))
            .and(header("api-key", QDRANT_API_KEY))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let err = authed_client(&server.uri())
            .hybrid_search(
                COLLECTION,
                &[0.1],
                &SparseVector {
                    indices: vec![1],
                    values: vec![1.0],
                },
                5,
                10,
            )
            .await
            .expect_err("500 should propagate");

        match err {
            QdrantError::Api { status, body } => {
                assert_eq!(status, 500);
                assert_eq!(body, "boom");
            }
            other => panic!("expected QdrantError::Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_upsert_points_sends_wait_true() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/collections/lab-tools/points"))
            .and(query_param("wait", "true"))
            .and(header("api-key", QDRANT_API_KEY))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "result": {} })))
            .mount(&server)
            .await;

        authed_client(&server.uri())
            .upsert_points(COLLECTION, &[point()])
            .await
            .expect("upsert should succeed");
    }

    #[tokio::test]
    async fn test_upsert_points_empty_is_noop() {
        let server = MockServer::start().await;
        authed_client(&server.uri())
            .upsert_points(COLLECTION, &[])
            .await
            .expect("empty upsert should be a no-op");

        let requests = server.received_requests().await.expect("recorded requests");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn test_delete_by_filter_sends_wait_true() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/lab-tools/points/delete"))
            .and(query_param("wait", "true"))
            .and(header("api-key", QDRANT_API_KEY))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "result": {} })))
            .mount(&server)
            .await;

        authed_client(&server.uri())
            .delete_by_filter(COLLECTION, json!({ "must": [] }))
            .await
            .expect("delete should succeed");
    }
}
