//! Deterministic generated documentation artifacts.

mod action_catalog;
mod artifacts;
mod projection;
mod render;
mod routes;
mod types;

pub use artifacts::{check, generate};

#[cfg(test)]
pub(crate) use projection::secret_example_is_suspicious;
