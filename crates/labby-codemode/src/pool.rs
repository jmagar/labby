//! Code Mode warm-runner pool (Perf H1).
//!
//! Pools the **OS process** of the Code Mode runner, not the JS runtime. Each
//! pooled runner builds a FRESH `javy::Runtime` per `Start` (runner-side), so JS
//! state isolation is guaranteed by construction — a brand-new runtime has no
//! globals, no `__labPendingToolCalls`, and no captured data from a prior
//! caller. Pooling amortizes only the expensive `fork()` + process startup.
//!
//! Concurrency model: a bounded set of long-lived runners, one execution per
//! runner at a time. `size` permits gate access to pooled runners; when all are
//! busy the caller spawns a bounded ephemeral (overflow) runner instead of
//! queueing unboundedly. A runner that crashes, times out, or reaches the
//! recycle-after-K threshold is evicted and a fresh one is spawned.
//!
//! Kill switch: `LAB_CODE_MODE_POOL_SIZE=0` disables pooling entirely and the
//! drive layer falls back to the historical spawn-per-execution path.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::error::ToolError;

use super::pool::config::PoolConfig;
use super::pool::runner_handle::PooledRunner;

pub(crate) mod config;
#[cfg(windows)]
pub(crate) mod job_guard;
pub(crate) mod runner_handle;

/// Host-configurable runner re-invocation: the program to exec and the args to
/// pass it. The pool re-execs this per spawned runner.
///
/// The default is the current executable plus the canonical
/// `["internal", "code-mode-runner"]` args, so a labby binary hosts its own
/// runner; a different host binary can supply its own program/args seam.
#[derive(Debug, Clone)]
pub struct RunnerSpawn {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
}

impl RunnerSpawn {
    pub fn try_default() -> Result<Self, ToolError> {
        let program = std::env::current_exe().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to resolve current executable for Code Mode runner: {err}"),
        })?;
        Ok(Self {
            program,
            args: vec!["internal".to_string(), "code-mode-runner".to_string()],
        })
    }
}

/// A bounded pool of long-lived Code Mode runner processes.
///
/// Slot ownership is tracked by an explicit free-list of indices rather than by
/// scanning mutex states, so two concurrent checkouts can never select the same
/// slot. A slot index is popped on checkout and pushed back on lease finalize.
pub struct RunnerPool {
    config: PoolConfig,
    /// Host-supplied runner re-invocation (program + args).
    spawn: RunnerSpawn,
    /// One slot per pooled runner. `None` = empty slot to (re)spawn into. The
    /// outer `Mutex` is only ever held briefly to move the runner in/out; the
    /// free-list guarantees single-owner access for the lease lifetime.
    slots: Vec<Arc<Mutex<Option<PooledRunner>>>>,
    /// Indices of currently-idle slots. Popping is the atomic "claim a slot"
    /// operation; the popped index is held by the lease and pushed back on
    /// finalize.
    free_slots: Arc<StdMutex<VecDeque<usize>>>,
    /// Bounds simultaneous ephemeral (overflow) runners when the pool is saturated.
    overflow_permits: Arc<Semaphore>,
}

impl RunnerPool {
    /// Build a pool from the environment-derived config and the default runner
    /// spawn seam (`current_exe()` + `["internal", "code-mode-runner"]`). When
    /// pooling is disabled (`size == 0`) the pool holds no slots and every
    /// checkout returns an ephemeral runner — i.e. the spawn-per-execution
    /// fallback.
    pub fn from_env() -> Result<Self, ToolError> {
        Ok(Self::with_config_and_spawn(
            PoolConfig::from_env(),
            RunnerSpawn::try_default()?,
        ))
    }

    /// Build a pool with an explicit, host-supplied runner spawn seam.
    pub fn with_spawn(spawn: RunnerSpawn) -> Self {
        Self::with_config_and_spawn(PoolConfig::from_env(), spawn)
    }

    #[cfg(test)]
    pub(crate) fn with_config(config: PoolConfig) -> Self {
        Self::with_config_and_spawn(
            config,
            RunnerSpawn::try_default().expect("test process must expose current executable"),
        )
    }

    fn with_config_and_spawn(config: PoolConfig, spawn: RunnerSpawn) -> Self {
        let slots = (0..config.size)
            .map(|_| Arc::new(Mutex::new(None)))
            .collect::<Vec<_>>();
        let free_slots = (0..config.size).collect::<VecDeque<_>>();
        // Overflow permits bound simultaneous ephemeral runners. Honor explicit
        // low values even when pooling is disabled so the kill switch can also
        // reduce resource pressure; only avoid a zero-permit deadlock.
        let overflow = config.max_overflow.max(1);
        Self {
            config,
            spawn,
            slots,
            free_slots: Arc::new(StdMutex::new(free_slots)),
            overflow_permits: Arc::new(Semaphore::new(overflow)),
        }
    }

    #[cfg(test)]
    pub(crate) fn config(&self) -> PoolConfig {
        self.config
    }

    #[cfg(test)]
    pub(crate) fn available_overflow_permits(&self) -> usize {
        self.overflow_permits.available_permits()
    }

    /// Check out a runner for one execution.
    ///
    /// Returns a [`RunnerLease`] that owns exclusive access to one runner for the
    /// duration of a single execution. Claims an idle pooled slot first; on
    /// saturation spawns a bounded ephemeral runner. The lease MUST be finalized
    /// via [`RunnerLease::release`] / [`RunnerLease::evict`] (or dropped, which
    /// evicts) so the slot index returns to the free-list.
    pub(crate) async fn checkout(&self) -> Result<RunnerLease, ToolError> {
        // Pooled fast path: atomically claim a free slot index.
        if !self.config.is_disabled() {
            let claimed = self.free_slots.lock().expect("free-list lock").pop_front();
            if let Some(index) = claimed {
                let runner = self.take_or_spawn_slot(index).await;
                match runner {
                    Ok(runner) => {
                        return Ok(RunnerLease::pooled(
                            Arc::clone(&self.slots[index]),
                            Arc::clone(&self.free_slots),
                            index,
                            runner,
                            self.config,
                        ));
                    }
                    Err(err) => {
                        // Spawn failed; return the slot to the free-list so it is
                        // retried later rather than permanently lost.
                        self.free_slots
                            .lock()
                            .expect("free-list lock")
                            .push_back(index);
                        return Err(err);
                    }
                }
            }
        }

        // Saturated (or disabled): spawn a bounded ephemeral runner.
        let permit = Arc::clone(&self.overflow_permits)
            .acquire_owned()
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: "Code Mode runner pool overflow semaphore closed".to_string(),
            })?;
        tracing::debug!(
            surface = "dispatch",
            service = "code_mode",
            action = "pool.overflow",
            pool_size = self.config.size,
            "pool saturated; spawning ephemeral Code Mode runner"
        );
        let runner = PooledRunner::spawn(&self.spawn)?;
        Ok(RunnerLease::ephemeral(runner, permit))
    }

    /// Take the runner out of a claimed slot, spawning a fresh one if the slot is
    /// empty (first use or post-eviction).
    async fn take_or_spawn_slot(&self, index: usize) -> Result<PooledRunner, ToolError> {
        let mut guard = self.slots[index].lock().await;
        match guard.take() {
            Some(runner) => Ok(runner),
            None => PooledRunner::spawn(&self.spawn),
        }
    }

    /// Test-only checkout that spawns stub (non-protocol) runners so the pool's
    /// lease / free-list / recycle / eviction bookkeeping and PID reuse can be
    /// exercised without the real labby binary.
    #[cfg(test)]
    pub(crate) async fn checkout_stub(&self) -> Result<RunnerLease, ToolError> {
        if !self.config.is_disabled() {
            let claimed = self.free_slots.lock().expect("free-list lock").pop_front();
            if let Some(index) = claimed {
                let mut guard = self.slots[index].lock().await;
                let runner = match guard.take() {
                    Some(runner) => runner,
                    None => PooledRunner::spawn_stub()?,
                };
                drop(guard);
                return Ok(RunnerLease::pooled(
                    Arc::clone(&self.slots[index]),
                    Arc::clone(&self.free_slots),
                    index,
                    runner,
                    self.config,
                ));
            }
        }
        let permit = Arc::clone(&self.overflow_permits)
            .acquire_owned()
            .await
            .expect("overflow semaphore open");
        Ok(RunnerLease::ephemeral(PooledRunner::spawn_stub()?, permit))
    }
}

/// Exclusive lease on one runner for a single execution.
///
/// On `release` a pooled runner is returned to its slot (unless it hit the
/// recycle threshold); an ephemeral runner is simply dropped (killed). On
/// `evict` the runner is always dropped and a pooled slot is left empty to be
/// respawned on next checkout. Dropping the lease without finalizing also
/// evicts (the runner is dropped and the slot left empty) — fail-safe.
pub(crate) struct RunnerLease {
    kind: LeaseKind,
    runner: Option<PooledRunner>,
}

enum LeaseKind {
    Pooled {
        slot: Arc<Mutex<Option<PooledRunner>>>,
        free_slots: Arc<StdMutex<VecDeque<usize>>>,
        index: usize,
        recycle_after: u64,
        /// Set once the slot index has been returned to the free-list so neither
        /// an explicit finalize nor the `Drop` fail-safe pushes it twice.
        returned: bool,
    },
    Ephemeral {
        _permit: OwnedSemaphorePermit,
    },
}

impl RunnerLease {
    fn pooled(
        slot: Arc<Mutex<Option<PooledRunner>>>,
        free_slots: Arc<StdMutex<VecDeque<usize>>>,
        index: usize,
        runner: PooledRunner,
        config: PoolConfig,
    ) -> Self {
        Self {
            kind: LeaseKind::Pooled {
                slot,
                free_slots,
                index,
                recycle_after: config.recycle_after,
                returned: false,
            },
            runner: Some(runner),
        }
    }

    fn ephemeral(runner: PooledRunner, permit: OwnedSemaphorePermit) -> Self {
        Self {
            kind: LeaseKind::Ephemeral { _permit: permit },
            runner: Some(runner),
        }
    }

    /// Mutable access to the leased runner for driving one execution.
    pub(crate) fn runner_mut(&mut self) -> &mut PooledRunner {
        self.runner
            .as_mut()
            .expect("runner present for the lease lifetime")
    }

    /// Whether this lease is backed by a pooled (long-lived, reused) runner.
    #[cfg(test)]
    pub(crate) fn is_pooled(&self) -> bool {
        matches!(self.kind, LeaseKind::Pooled { .. })
    }

    /// Return the runner after a clean execution.
    ///
    /// A pooled runner is parked back in its slot unless it has reached the
    /// recycle-after-K threshold, in which case it is dropped (killed) and the
    /// slot left empty so the next checkout spawns a fresh process. Either way
    /// the slot index returns to the free-list. An ephemeral runner is dropped.
    pub(crate) async fn release(mut self) {
        let runner = self.runner.take();
        match (&mut self.kind, runner) {
            (
                LeaseKind::Pooled {
                    slot,
                    free_slots,
                    index,
                    recycle_after,
                    returned,
                },
                Some(mut runner),
            ) => {
                runner.executions = runner.executions.saturating_add(1);
                if runner.executions >= *recycle_after {
                    tracing::debug!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "pool.recycle",
                        executions = runner.executions,
                        "recycling pooled Code Mode runner after threshold"
                    );
                    // Drop (kill) the runner; leave the slot empty to respawn.
                    drop(runner);
                } else {
                    *slot.lock().await = Some(runner);
                }
                return_slot(free_slots, *index, returned);
            }
            // Ephemeral runner, or pooled with no runner (already taken): drop.
            (_, runner) => drop(runner),
        }
    }

    /// Discard the runner after a crash, timeout, or protocol fault.
    ///
    /// The runner is always dropped (killed); a pooled slot is left empty (the
    /// runner is never returned) so the next checkout spawns a fresh replacement.
    /// The slot index still returns to the free-list so the slot is reusable.
    pub(crate) fn evict(mut self) {
        drop(self.runner.take());
        if let LeaseKind::Pooled {
            free_slots,
            index,
            returned,
            ..
        } = &mut self.kind
        {
            return_slot(free_slots, *index, returned);
        }
    }
}

/// Return a claimed slot index to the free-list exactly once.
fn return_slot(free_slots: &Arc<StdMutex<VecDeque<usize>>>, index: usize, returned: &mut bool) {
    if *returned {
        return;
    }
    *returned = true;
    free_slots.lock().expect("free-list lock").push_back(index);
}

impl Drop for RunnerLease {
    fn drop(&mut self) {
        // Fail-safe: if neither release nor evict ran (e.g. early `?`), the
        // runner is dropped here (killed) and the pooled slot is left empty (we
        // never parked the runner). This is the conservative eviction path —
        // never park a runner whose execution state we did not verify. The slot
        // index still returns to the free-list so the slot is reusable.
        drop(self.runner.take());
        if let LeaseKind::Pooled {
            free_slots,
            index,
            returned,
            ..
        } = &mut self.kind
        {
            return_slot(free_slots, *index, returned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::config::PoolConfig;
    use super::*;

    fn cfg(size: usize, recycle_after: u64, max_overflow: usize) -> PoolConfig {
        PoolConfig {
            size,
            recycle_after,
            max_overflow,
        }
    }

    /// A pooled runner returned via `release` is parked in its slot and the SAME
    /// process is handed out on the next checkout (PID reuse) — the core warm
    /// behavior. Crucially, the parent-side bookkeeping reuses the process
    /// rather than spawning a fresh one each time.
    #[tokio::test]
    async fn pool_reuses_the_same_process_across_checkouts() {
        let pool = RunnerPool::with_config(cfg(1, 100, 4));
        let mut lease = pool.checkout_stub().await.expect("checkout");
        assert!(lease.is_pooled(), "size>0 → pooled lease");
        let first_pid = lease.runner_mut().child_pid;
        lease.release().await;

        let mut lease2 = pool.checkout_stub().await.expect("re-checkout");
        let second_pid = lease2.runner_mut().child_pid;
        assert_eq!(
            first_pid, second_pid,
            "a released pooled runner must be reused (same PID) on the next checkout"
        );
        lease2.release().await;
    }

    /// Evicting a runner kills it and leaves the slot empty; the next checkout
    /// spawns a FRESH process (different PID) — crash/fault recovery.
    #[tokio::test]
    async fn pool_respawns_after_eviction() {
        let pool = RunnerPool::with_config(cfg(1, 100, 4));
        let mut lease = pool.checkout_stub().await.expect("checkout");
        let first_pid = lease.runner_mut().child_pid;
        // Simulate a crash/fault: evict instead of release.
        lease.evict();

        let mut lease2 = pool.checkout_stub().await.expect("re-checkout");
        let second_pid = lease2.runner_mut().child_pid;
        assert_ne!(
            first_pid, second_pid,
            "an evicted runner must be replaced by a freshly spawned process"
        );
        lease2.release().await;
    }

    /// After K executions a pooled runner is recycled: the next checkout spawns a
    /// fresh process. With `recycle_after = 2`, the 3rd checkout must see a new PID.
    #[tokio::test]
    async fn pool_recycles_runner_after_k_executions() {
        let pool = RunnerPool::with_config(cfg(1, 2, 4));
        let mut l1 = pool.checkout_stub().await.expect("checkout 1");
        let pid1 = l1.runner_mut().child_pid;
        l1.release().await; // executions: 1

        let mut l2 = pool.checkout_stub().await.expect("checkout 2");
        let pid2 = l2.runner_mut().child_pid;
        assert_eq!(pid1, pid2, "same runner before the recycle threshold");
        l2.release().await; // executions: 2 → hits recycle_after, runner dropped

        let mut l3 = pool.checkout_stub().await.expect("checkout 3");
        let pid3 = l3.runner_mut().child_pid;
        assert_ne!(
            pid2, pid3,
            "runner must be recycled (fresh PID) after K executions"
        );
        l3.release().await;
    }

    /// Concurrency: N pooled slots serve N simultaneous leases with N distinct
    /// processes (they do not serialize onto one runner). All three leases are
    /// acquired and held at the same time, so the pool cannot have handed any
    /// pair the same process.
    #[tokio::test]
    async fn pool_serves_concurrent_leases_with_distinct_runners() {
        let pool = RunnerPool::with_config(cfg(3, 100, 4));
        let a = pool.checkout_stub().await.expect("a");
        let b = pool.checkout_stub().await.expect("b");
        let c = pool.checkout_stub().await.expect("c");
        let mut leases = [a, b, c];
        assert!(
            leases.iter().all(RunnerLease::is_pooled),
            "all three concurrent leases must be served from pooled slots, not overflow"
        );
        let pids: std::collections::HashSet<_> = leases
            .iter_mut()
            .map(|l| l.runner_mut().child_pid)
            .collect();
        assert_eq!(
            pids.len(),
            3,
            "three concurrent pooled leases must hold three distinct processes \
             (N runners serve N concurrent executions without serializing)"
        );
        for l in leases {
            l.release().await;
        }
    }

    /// Fallback parity: the kill switch (`size == 0`) makes every checkout
    /// spawn-per-execution (ephemeral, distinct PIDs, never reused), exactly the
    /// pre-pool behavior. With pooling enabled, the same process is reused. This
    /// asserts the only behavioral difference the switch introduces is reuse —
    /// both paths hand out a working, live runner lease.
    #[tokio::test]
    async fn kill_switch_matches_spawn_per_execution_behavior() {
        // Disabled: each checkout is a fresh process.
        let disabled = RunnerPool::with_config(cfg(0, 100, 8));
        let mut d1 = disabled.checkout_stub().await.expect("d1");
        let d1_pid = d1.runner_mut().child_pid;
        d1.release().await;
        let mut d2 = disabled.checkout_stub().await.expect("d2");
        let d2_pid = d2.runner_mut().child_pid;
        d2.release().await;
        assert_ne!(
            d1_pid, d2_pid,
            "kill switch must spawn a fresh process per execution (no reuse)"
        );

        // Enabled: the process is reused.
        let enabled = RunnerPool::with_config(cfg(1, 100, 8));
        let mut e1 = enabled.checkout_stub().await.expect("e1");
        let e1_pid = e1.runner_mut().child_pid;
        e1.release().await;
        let mut e2 = enabled.checkout_stub().await.expect("e2");
        let e2_pid = e2.runner_mut().child_pid;
        e2.release().await;
        assert_eq!(e1_pid, e2_pid, "enabled pool reuses the process");
    }

    /// When all pooled slots are busy, an extra checkout is served by a bounded
    /// ephemeral (overflow) runner rather than blocking forever or growing
    /// unbounded.
    #[tokio::test]
    async fn pool_overflows_to_ephemeral_runner_when_saturated() {
        let pool = RunnerPool::with_config(cfg(1, 100, 2));
        let held = pool.checkout_stub().await.expect("hold the only slot");
        assert!(held.is_pooled());

        // Pool is saturated (1 slot, held). The next checkout must still succeed
        // via an ephemeral runner.
        let overflow = pool.checkout_stub().await.expect("overflow checkout");
        assert!(
            !overflow.is_pooled(),
            "a saturated pool must serve overflow via an ephemeral (non-pooled) runner"
        );
        overflow.release().await; // ephemeral → dropped
        held.release().await;
    }

    /// The kill switch (`size == 0`) disables pooling: every checkout is an
    /// ephemeral runner, matching the spawn-per-execution fallback.
    #[tokio::test]
    async fn pool_disabled_serves_only_ephemeral_runners() {
        let pool = RunnerPool::with_config(cfg(0, 100, 2));
        assert!(pool.config().is_disabled());
        assert_eq!(
            pool.available_overflow_permits(),
            2,
            "disabled pooling must honor configured overflow concurrency"
        );
        let mut a = pool.checkout_stub().await.expect("a");
        let mut b = pool.checkout_stub().await.expect("b");
        assert!(!a.is_pooled(), "disabled pool → ephemeral lease");
        assert!(!b.is_pooled(), "disabled pool → ephemeral lease");
        assert_ne!(
            a.runner_mut().child_pid,
            b.runner_mut().child_pid,
            "each disabled-pool checkout spawns a distinct process"
        );
        a.release().await;
        b.release().await;
    }

    #[tokio::test]
    async fn disabled_pool_respects_low_overflow_limit() {
        let pool = RunnerPool::with_config(cfg(0, 100, 1));
        assert!(pool.config().is_disabled());
        assert_eq!(
            pool.available_overflow_permits(),
            1,
            "disabled pooling must not inflate an explicit low overflow limit"
        );
    }

    #[tokio::test]
    async fn disabled_pool_raises_zero_overflow_to_one_permit() {
        let pool = RunnerPool::with_config(cfg(0, 100, 0));
        assert!(pool.config().is_disabled());
        assert_eq!(
            pool.available_overflow_permits(),
            1,
            "zero overflow is raised only enough to avoid deadlock"
        );
    }

    /// Dropping a lease without finalizing is fail-safe: the slot index returns
    /// to the free-list so the pool is not permanently starved, and the runner is
    /// killed (a fresh one spawns next checkout).
    #[tokio::test]
    async fn dropping_a_lease_returns_the_slot_and_evicts_the_runner() {
        let pool = RunnerPool::with_config(cfg(1, 100, 4));
        let first_pid = {
            let mut lease = pool.checkout_stub().await.expect("checkout");
            let pid = lease.runner_mut().child_pid;
            // Drop without release/evict (e.g. an early `?`).
            drop(lease);
            pid
        };
        // The slot must be reusable, and the runner replaced.
        let mut lease2 = pool
            .checkout_stub()
            .await
            .expect("slot reusable after drop");
        assert_ne!(
            first_pid,
            lease2.runner_mut().child_pid,
            "dropped lease must evict the runner (fresh PID next checkout)"
        );
        lease2.release().await;
    }
}
