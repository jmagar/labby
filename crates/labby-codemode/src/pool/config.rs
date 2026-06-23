//! Configuration knobs and kill switch for the Code Mode warm-runner pool.
//!
//! All knobs are read from the environment with conservative defaults so the
//! pool is safe by construction. The pool can be disabled entirely (kill
//! switch) to fall back to the historical spawn-per-execution path with
//! byte-identical behavior.

/// Pool size env var. `0` disables pooling (kill switch → spawn-per-execution).
pub(crate) const POOL_SIZE_ENV: &str = "LAB_CODE_MODE_POOL_SIZE";
/// Recycle-after-K env var: kill+respawn a runner after K executions.
pub(crate) const RECYCLE_AFTER_ENV: &str = "LAB_CODE_MODE_POOL_RECYCLE_AFTER";
/// Max concurrent ephemeral (overflow) runners spawned when the pool is saturated.
pub(crate) const MAX_OVERFLOW_ENV: &str = "LAB_CODE_MODE_POOL_MAX_OVERFLOW";

/// Conservative default pool size. Small enough to keep idle RSS bounded while
/// still absorbing typical search+execute bursts without serializing.
const DEFAULT_POOL_SIZE: usize = 2;
/// Hard ceiling on configured pool size so a typo cannot fork hundreds of
/// long-lived 64-MiB-capable processes.
const MAX_POOL_SIZE: usize = 16;
/// Default executions before a pooled runner is recycled (kill+respawn). Cheap
/// insurance against native-side fragmentation/leaks in the long-lived process.
const DEFAULT_RECYCLE_AFTER: u64 = 100;
/// Default cap on simultaneous overflow (ephemeral) runners. Bounds total
/// concurrent runner processes to `pool_size + max_overflow`.
const DEFAULT_MAX_OVERFLOW: usize = 8;

/// Resolved, validated pool configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolConfig {
    /// Number of long-lived pooled runners. `0` means pooling is disabled.
    pub(crate) size: usize,
    /// Executions a pooled runner serves before being recycled.
    pub(crate) recycle_after: u64,
    /// Max simultaneous ephemeral runners spawned when the pool is saturated.
    pub(crate) max_overflow: usize,
}

impl PoolConfig {
    /// Read the pool configuration from the environment, clamping to safe bounds.
    pub(crate) fn from_env() -> Self {
        let size = parse_env(POOL_SIZE_ENV, DEFAULT_POOL_SIZE).min(MAX_POOL_SIZE);
        let recycle_after = parse_env_u64(RECYCLE_AFTER_ENV, DEFAULT_RECYCLE_AFTER).max(1);
        let max_overflow = parse_env(MAX_OVERFLOW_ENV, DEFAULT_MAX_OVERFLOW);
        Self {
            size,
            recycle_after,
            max_overflow,
        }
    }

    /// True when pooling is disabled (kill switch): every execution spawns a
    /// fresh one-shot runner exactly as before this feature landed.
    pub(crate) fn is_disabled(&self) -> bool {
        self.size == 0
    }
}

fn parse_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn parse_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pool_config_is_conservative() {
        // No env vars set in this test process for these names by default.
        let cfg = PoolConfig {
            size: DEFAULT_POOL_SIZE,
            recycle_after: DEFAULT_RECYCLE_AFTER,
            max_overflow: DEFAULT_MAX_OVERFLOW,
        };
        assert!(!cfg.is_disabled());
        assert_eq!(cfg.size, 2);
        assert_eq!(cfg.recycle_after, 100);
    }

    #[test]
    fn zero_size_disables_pooling() {
        let cfg = PoolConfig {
            size: 0,
            recycle_after: 1,
            max_overflow: 0,
        };
        assert!(cfg.is_disabled());
    }

    #[test]
    fn size_is_clamped_to_max() {
        assert_eq!(DEFAULT_POOL_SIZE.min(MAX_POOL_SIZE), DEFAULT_POOL_SIZE);
        assert_eq!(1000usize.min(MAX_POOL_SIZE), MAX_POOL_SIZE);
    }
}
