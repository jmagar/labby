//! HuggingFace Text Embeddings Inference client.

pub mod client;
pub mod error;

pub use client::{QUERY_INSTRUCTION, TeiClient};
pub use error::TeiError;
