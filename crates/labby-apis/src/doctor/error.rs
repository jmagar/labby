//! Doctor service errors.

use crate::core::error::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    #[error(transparent)]
    Api(#[from] ApiError),
}
