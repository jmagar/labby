#[derive(Debug, Clone)]
pub(crate) struct StateWorkspaceLimits {
    pub(crate) max_file_bytes: usize,
    pub(crate) max_total_bytes: u64,
    pub(crate) max_entries: u64,
    pub(crate) max_result_bytes: usize,
}

impl Default for StateWorkspaceLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_entries: 10_000,
            max_result_bytes: 1024 * 1024,
        }
    }
}
