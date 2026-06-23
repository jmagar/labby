use std::collections::HashMap;

use labby_runtime::error::ToolError;

pub(super) fn read_env_values(
    path: &std::path::Path,
) -> Result<HashMap<String, String>, ToolError> {
    Ok(dotenvy::from_path_iter(path)
        .ok()
        .map(|iter| iter.filter_map(Result::ok).collect())
        .unwrap_or_default())
}
