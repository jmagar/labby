//! Tests for upstream-proxy result normalization. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::normalize_upstream_result;
use crate::mcp::envelope::build_error;
use rmcp::model::{CallToolResult, Content};

#[test]
fn normalize_upstream_result_preserves_user_errors_without_poisoning_health() {
    let upstream = CallToolResult::error(vec![Content::text(
        build_error("radarr", "movie.add", "missing_param", "need title").to_string(),
    )]);

    let (_, kind, counts_as_failure) = normalize_upstream_result("radarr", "call_tool", upstream);

    assert_eq!(kind, "missing_param");
    assert!(!counts_as_failure);
}
