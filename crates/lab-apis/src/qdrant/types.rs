//! Qdrant wire types used by the client.

use serde::{Deserialize, Serialize};

/// Sparse vector in Qdrant's `{indices, values}` wire format.
///
/// Qdrant applies IDF correction server-side when the collection's sparse vector
/// config has `"modifier": "idf"`. Clients emit log-normalized TF weights:
/// `ln(1 + raw_count)`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
}

impl SparseVector {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// A point to upsert with named dense + sparse vectors.
#[derive(Debug, Clone)]
pub struct UpsertPoint {
    /// Stable numeric ID. Callers use a content-hash over the point's natural key.
    pub id: u64,
    /// Dense embedding (must match the collection's `dense` vector dimension).
    pub dense: Vec<f32>,
    /// BM42-style sparse vector.
    pub sparse: SparseVector,
    /// Arbitrary JSON payload stored alongside the vector.
    pub payload: serde_json::Value,
}

/// A single hit from a Qdrant search or query response.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub id: serde_json::Value,
    pub score: f64,
    #[serde(default)]
    pub payload: serde_json::Value,
}

// Internal deserialization types — Qdrant's response envelope.

#[derive(Deserialize)]
pub(crate) struct QueryPoints {
    pub points: Vec<SearchHit>,
}

#[derive(Deserialize)]
pub(crate) struct QueryResponse {
    pub result: QueryPoints,
}
