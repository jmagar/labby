//! Qdrant HTTP client — collection management, upsert, hybrid search.

use std::sync::LazyLock;
use std::time::Duration;

use super::error::QdrantError;
use super::types::{QueryResponse, SearchHit, SparseVector, UpsertPoint};
use serde::Serialize;
use serde_json::json;

/// Shared HTTP client. `reqwest::Client` owns a connection pool; constructing one
/// per call exhausts sockets and bypasses pooling.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(90))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build shared reqwest client for Qdrant")
});

pub struct QdrantClient {
    base_url: String,
}

impl QdrantClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
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
        let resp = HTTP_CLIENT.get(&check_url).send().await?;
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
        let resp = HTTP_CLIENT.put(&create_url).json(&body).send().await?;
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
        let resp = HTTP_CLIENT
            .put(&url)
            .json(&json!({ "points": bodies }))
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
        let resp = HTTP_CLIENT.post(&url).json(&body).send().await?;
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
        let resp = HTTP_CLIENT.post(&url).json(&body).send().await?;
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
        let resp = HTTP_CLIENT.post(&url).json(&body).send().await?;
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
        let resp = HTTP_CLIENT
            .post(&url)
            .json(&json!({ "filter": filter }))
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
