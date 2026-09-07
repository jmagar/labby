use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::ErrorData;
use rmcp::model::PaginatedRequestParams;

use labby_runtime::agent_error::AgentErrorContext;

use crate::mcp::agent_error::invalid_params as invalid_params_agent_error;

pub(crate) const MCP_LIST_PAGE_SIZE: usize = 100;
pub(crate) const MCP_RETAINED_LIST_ITEM_CAP: usize = MCP_LIST_PAGE_SIZE + 1;

static CATALOG_SNAPSHOT_REVISION: AtomicU64 = AtomicU64::new(1);

/// Mint an opaque revision that identifies one exact retained catalog snapshot.
///
/// The revision only needs to be unique for the lifetime of this process: after
/// a restart the snapshot store is empty and old cursors correctly fail closed.
pub(crate) fn next_catalog_snapshot_revision() -> String {
    format!(
        "{:016x}",
        CATALOG_SNAPSHOT_REVISION.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) struct PageCollector<T> {
    start: usize,
    expected_revision: Option<String>,
    bound_revision: Option<String>,
    seen: usize,
    page: Vec<T>,
    has_next: bool,
}

impl<T> PageCollector<T> {
    pub(crate) fn new(request: Option<PaginatedRequestParams>) -> Result<Self, ErrorData> {
        let (start, expected_revision) = match request.and_then(|request| request.cursor) {
            Some(cursor) => parse_cursor(&cursor)?,
            None => (0, None),
        };
        Ok(Self {
            start,
            expected_revision,
            bound_revision: None,
            seen: 0,
            page: Vec::with_capacity(MCP_LIST_PAGE_SIZE),
            has_next: false,
        })
    }

    /// Revision embedded in a versioned cursor supplied by the caller.
    ///
    /// Catalog handlers that retain a server-side snapshot can use this before
    /// rebuilding anything expensive, so a cursor page resumes the exact
    /// result set that produced it instead of re-running discovery.
    pub(crate) fn expected_revision(&self) -> Option<&str> {
        self.expected_revision.as_deref()
    }

    pub(crate) const fn start_offset(&self) -> usize {
        self.start
    }

    /// Bind this page to the complete result set that produced it.
    ///
    /// A subsequent page must present the same revision embedded in the cursor;
    /// otherwise an offset could resume against a different catalog and silently
    /// duplicate or omit entries.
    pub(crate) fn bind_revision(&mut self, revision: &str) -> Result<(), ErrorData> {
        if self.start > 0 && self.expected_revision.is_none() {
            return Err(invalid_cursor(
                "cursor must include the result-set revision",
            ));
        }
        if self
            .expected_revision
            .as_deref()
            .is_some_and(|expected| expected != revision)
        {
            return Err(invalid_cursor(
                "cursor was issued for a different result-set revision",
            ));
        }
        self.bound_revision = Some(revision.to_string());
        Ok(())
    }

    pub(crate) fn accept(&mut self, item: T) {
        if self.finished() {
            return;
        }
        if self.seen < self.start {
            self.seen += 1;
            return;
        }
        if self.page.len() < MCP_LIST_PAGE_SIZE {
            self.page.push(item);
            self.seen += 1;
            return;
        }
        self.has_next = true;
        self.seen += 1;
    }

    pub(crate) fn finished(&self) -> bool {
        self.has_next
    }

    pub(crate) fn finish(self) -> Result<(Vec<T>, Option<String>), ErrorData> {
        if self.seen < self.start {
            return Err(invalid_cursor("cursor is past the end of the result set"));
        }
        let next_cursor = self.has_next.then(|| {
            let offset = self.start + self.page.len();
            self.bound_revision.as_ref().map_or_else(
                || offset.to_string(),
                |revision| format!("v1:{offset}:{revision}"),
            )
        });
        Ok((self.page, next_cursor))
    }
}

/// First-page collector for catalogs whose construction performs live I/O.
///
/// The ordinary `PageCollector` stops after one page plus lookahead. For a live
/// upstream catalog that would force every follow-up cursor page to repeat the
/// discovery fan-out. This collector still bounds the wire page, but continues
/// walking once so the complete result set can be retained in shared route
/// state and resumed without I/O.
pub(crate) struct CatalogSnapshotCollector<T> {
    page: PageCollector<T>,
    catalog: Vec<T>,
}

impl<T: Clone> CatalogSnapshotCollector<T> {
    pub(crate) fn new(page: PageCollector<T>) -> Self {
        Self {
            page,
            catalog: Vec::new(),
        }
    }

    pub(crate) fn accept(&mut self, item: T) {
        self.catalog.push(item.clone());
        self.page.accept(item);
    }

    /// A snapshot build deliberately consumes the complete live catalog.
    pub(crate) const fn finished(&self) -> bool {
        false
    }

    pub(crate) fn bind_revision(&mut self, revision: &str) -> Result<(), ErrorData> {
        self.page.bind_revision(revision)
    }

    pub(crate) fn finish(self) -> Result<(Vec<T>, Option<String>, Vec<T>), ErrorData> {
        let (page, next_cursor) = self.page.finish()?;
        Ok((page, next_cursor, self.catalog))
    }
}

#[cfg(test)]
fn try_collect_page<T, I>(
    items: I,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData>
where
    I: IntoIterator<Item = T>,
{
    let mut collector = PageCollector::new(request)?;
    for item in items {
        collector.accept(item);
        if collector.finished() {
            break;
        }
    }
    collector.finish()
}

#[cfg(test)]
fn paginate_items<T>(
    items: Vec<T>,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData> {
    let start = match request.as_ref().and_then(|request| request.cursor.as_ref()) {
        Some(cursor) => parse_cursor(cursor)?.0,
        None => 0,
    };
    if start > items.len() {
        return Err(invalid_cursor("cursor is past the end of the result set"));
    }
    try_collect_page(items, request)
}

pub(crate) fn error_kind(error: &ErrorData) -> &'static str {
    match error
        .data
        .as_ref()
        .and_then(|data| data.get("kind"))
        .and_then(serde_json::Value::as_str)
    {
        Some("invalid_cursor") => "invalid_cursor",
        _ => "invalid_params",
    }
}

fn parse_cursor(cursor: &str) -> Result<(usize, Option<String>), ErrorData> {
    if let Some(versioned) = cursor.strip_prefix("v1:") {
        let Some((offset, revision)) = versioned.split_once(':') else {
            return Err(invalid_cursor(
                "versioned cursor must contain an offset and revision",
            ));
        };
        if revision.is_empty() || revision.contains(':') {
            return Err(invalid_cursor("cursor revision is malformed"));
        }
        let offset = offset
            .parse::<usize>()
            .map_err(|_| invalid_cursor("cursor must contain a non-negative integer offset"))?;
        return Ok((offset, Some(revision.to_string())));
    }
    cursor
        .parse::<usize>()
        .map(|offset| (offset, None))
        .map_err(|_| invalid_cursor("cursor must be a non-negative integer offset"))
}

pub(crate) fn invalid_cursor(message: &str) -> ErrorData {
    invalid_params_agent_error(
        "invalid_cursor",
        message,
        None,
        &AgentErrorContext::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_collector_stops_after_page_plus_lookahead() {
        let mut collector = PageCollector::new(None).expect("collector");
        let mut visited = 0;

        for item in 0..250 {
            visited += 1;
            collector.accept(item);
            if collector.finished() {
                break;
            }
        }

        let (page, next_cursor) = collector.finish().expect("page");
        assert_eq!(visited, MCP_LIST_PAGE_SIZE + 1);
        assert_eq!(page, (0..MCP_LIST_PAGE_SIZE).collect::<Vec<_>>());
        assert_eq!(next_cursor.as_deref(), Some("100"));
    }

    #[test]
    fn page_collector_counts_skipped_items_without_storing_them() {
        let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));
        let mut collector = PageCollector::new(Some(request)).expect("collector");
        let mut visited = 0;

        for item in 0..250 {
            visited += 1;
            collector.accept(item);
            if collector.finished() {
                break;
            }
        }

        let (page, next_cursor) = collector.finish().expect("page");
        assert_eq!(visited, 250);
        assert_eq!(page, (200..250).collect::<Vec<_>>());
        assert_eq!(next_cursor, None);
    }

    #[test]
    fn page_collector_rejects_cursor_past_end() {
        let request = PaginatedRequestParams::default().with_cursor(Some("4".to_string()));
        let mut collector = PageCollector::new(Some(request)).expect("collector");

        for item in 0..3 {
            collector.accept(item);
        }

        let err = collector.finish().expect_err("cursor past end");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
        let data = err.data.as_ref().expect("error data");
        assert_eq!(data["contract_version"], 1);
        assert_eq!(data["origin"], "discovery");
        assert_eq!(data["recovery"]["action"], "rediscover");
        assert_eq!(data["recovery"]["same_arguments"], "never");
        assert_eq!(data["side_effects"], "none_expected");
    }

    #[test]
    fn paginates_with_offset_cursor() {
        let items = (0..250).collect::<Vec<_>>();

        let (page, next_cursor) = paginate_items(items, None).expect("first page");

        assert_eq!(page.len(), MCP_LIST_PAGE_SIZE);
        assert_eq!(page[0], 0);
        assert_eq!(page[MCP_LIST_PAGE_SIZE - 1], 99);
        assert_eq!(next_cursor.as_deref(), Some("100"));
    }

    #[test]
    fn resumes_from_cursor() {
        let items = (0..250).collect::<Vec<_>>();
        let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));

        let (page, next_cursor) = paginate_items(items, Some(request)).expect("cursor page");

        assert_eq!(page, (200..250).collect::<Vec<_>>());
        assert_eq!(next_cursor, None);
    }

    #[test]
    fn rejects_invalid_cursor() {
        let request = PaginatedRequestParams::default().with_cursor(Some("nope".to_string()));

        let err = paginate_items(vec![1, 2, 3], Some(request)).expect_err("invalid cursor");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn catalog_snapshot_collector_keeps_complete_result_while_paging() {
        let page = PageCollector::new(None).expect("page collector");
        let mut collector = CatalogSnapshotCollector::new(page);
        for item in 0..250 {
            collector.accept(item);
        }
        let revision = next_catalog_snapshot_revision();
        collector
            .bind_revision(&revision)
            .expect("bind snapshot revision");

        let (page, cursor, catalog) = collector.finish().expect("snapshot page");

        assert_eq!(page, (0..MCP_LIST_PAGE_SIZE).collect::<Vec<_>>());
        assert_eq!(catalog, (0..250).collect::<Vec<_>>());
        assert_eq!(
            cursor.as_deref(),
            Some(format!("v1:100:{revision}").as_str())
        );
    }

    #[test]
    fn revision_bound_cursor_rejects_a_changed_catalog() {
        let mut first = PageCollector::new(None).expect("first collector");
        first
            .bind_revision("catalog-a")
            .expect("bind first revision");
        for item in 0..250 {
            first.accept(item);
            if first.finished() {
                break;
            }
        }
        let (_, cursor) = first.finish().expect("first page");
        let request = PaginatedRequestParams::default().with_cursor(cursor);
        let mut rebuilt = PageCollector::<usize>::new(Some(request)).expect("resume collector");

        let error = rebuilt
            .bind_revision("catalog-b")
            .expect_err("stale cursor must be rejected");

        assert_eq!(
            error.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn revision_bound_pagination_rejects_an_unversioned_continuation_cursor() {
        let request = PaginatedRequestParams::default().with_cursor(Some("100".to_string()));
        let mut collector = PageCollector::<usize>::new(Some(request)).expect("parse cursor");

        let error = collector
            .bind_revision("catalog-a")
            .expect_err("unversioned continuation must be rejected");

        assert_eq!(
            error.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn unversioned_cursor_remains_valid_for_unbound_resource_and_prompt_pagination() {
        let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));
        let mut collector = PageCollector::new(Some(request)).expect("parse legacy cursor");
        for item in 0..250 {
            collector.accept(item);
        }

        let (page, next_cursor) = collector.finish().expect("finish unbound page");

        assert_eq!(page, (200..250).collect::<Vec<_>>());
        assert_eq!(next_cursor, None);
    }
}
