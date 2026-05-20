//! Qdrant vector database client — upsert, hybrid search (dense + BM42 with RRF).

pub mod client;
pub mod error;
pub mod types;

pub use client::QdrantClient;
pub use error::QdrantError;
pub use types::{SearchHit, SparseVector, UpsertPoint};
