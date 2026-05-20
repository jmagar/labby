//! HuggingFace Text Embeddings Inference client.

pub mod client;
pub mod error;

pub use client::{TeiClient, QUERY_INSTRUCTION};
pub use error::TeiError;
