//! Cross-process spawn lock for stdio upstreams.
//!
//! stdio MCP servers launched via `npx -y <pkg>` / `uvx <pkg>` install into a
//! SHARED package cache (`~/.npm/_npx`, `~/.cache/uv`) on first cold spawn. When
//! two processes install the *same* package concurrently — the gateway daemon
//! racing a reconnect/probe attempt, or a separate `labby gateway test` / `mcp
//! enable` CLI process racing the daemon — they write the same cache directory
//! at once and corrupt it (partial `node_modules`, e.g. a missing peer dep),
//! after which the server crashes on startup before completing the MCP
//! handshake.
//!
//! The pool's in-process `lazy_connect_lock` (a `tokio::Mutex`) only serializes
//! connects WITHIN one process, so it cannot prevent the cross-process race.
//! This module adds an advisory **file lock** (`fd-lock`, the workspace's file
//! locking crate) keyed on a hash of the spawn command + args, held for the
//! entire connect (spawn → handshake → `list_tools`, the window during which the
//! install runs). Identical commands serialize across every process on the host;
//! once the cache is warm, each later acquire is a fast cache hit. Distinct
//! commands hash to distinct lock files and never contend, so the parallel
//! warmup stays parallel.
//!
//! It is deliberately keyed on the command itself, not on a hardcoded package
//! list — every current and future `npx`/`uvx`/binary stdio server is covered
//! automatically. Acquisition is best-effort: any filesystem/locking failure
//! logs at debug and proceeds without the lock rather than blocking a spawn.
//!
//! `fd-lock`'s `RwLockWriteGuard` borrows its `RwLock`, so the two cannot live
//! in one returnable struct. Callers therefore [`open`] the lock (a value they
//! own for the connect's duration) and pass `&mut` to [`acquire`]; the returned
//! guard is held as a sibling local and releases on drop. Declare the guard
//! AFTER the `RwLock` so it drops first (unlock before the file handle closes).

use fd_lock::{RwLock, RwLockWriteGuard};
use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// How long to wait for a contended lock before giving up and proceeding
/// without it. Generous enough to cover a slow cold `npm`/`uv` install.
const SPAWN_LOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// Poll interval while the lock is held by another process.
const SPAWN_LOCK_POLL: Duration = Duration::from_millis(100);

/// Open (but do not yet lock) the per-command lock file. Returns `None` on any
/// filesystem error so the caller proceeds best-effort without a lock. The
/// returned `RwLock` must outlive the guard returned by [`acquire`].
pub(super) fn open(command: &str, args: &[String]) -> Option<RwLock<File>> {
    let mut hasher = DefaultHasher::new();
    command.hash(&mut hasher);
    args.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    let dir = std::env::temp_dir().join("labby-spawn-locks");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::debug!(
            service = "upstream.pool",
            action = "upstream.spawn_lock",
            %error,
            "spawn lock dir create failed; proceeding without lock"
        );
        return None;
    }
    let path = dir.join(format!("{key}.lock"));
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => Some(RwLock::new(file)),
        Err(error) => {
            tracing::debug!(
                service = "upstream.pool",
                action = "upstream.spawn_lock",
                %error,
                "spawn lock file open failed; proceeding without lock"
            );
            None
        }
    }
}

/// Acquire the exclusive cross-process lock, polling until the holder releases
/// it or [`SPAWN_LOCK_TIMEOUT`] elapses. Returns `None` (proceed best-effort) if
/// `lock` is absent, on timeout, or on any locking error — never blocks a spawn.
pub(super) async fn acquire(lock: Option<&mut RwLock<File>>) -> Option<RwLockWriteGuard<'_, File>> {
    let lock = lock?;
    let deadline = Instant::now() + SPAWN_LOCK_TIMEOUT;

    // Wait until the lock looks free (or we time out), releasing the probe lock
    // between attempts. The probe guard is a temporary scoped to the `if`
    // condition, so it drops before the loop body runs. Keeping the final
    // acquisition OUTSIDE the loop avoids returning a borrow across the loop
    // back-edge (which the borrow checker rejects).
    let mut waited = false;
    while Instant::now() < deadline {
        // The probe guard is a temporary scoped to this `if` condition, so it
        // drops (releasing the lock) before the loop body runs.
        if lock.try_write().is_ok() {
            break;
        }
        waited = true;
        tokio::time::sleep(SPAWN_LOCK_POLL).await;
    }

    match lock.try_write() {
        Ok(guard) => {
            if waited {
                tracing::debug!(
                    service = "upstream.pool",
                    action = "upstream.spawn_lock",
                    "acquired contended spawn lock"
                );
            }
            Some(guard)
        }
        Err(error) => {
            tracing::warn!(
                service = "upstream.pool",
                action = "upstream.spawn_lock",
                %error,
                timeout_secs = SPAWN_LOCK_TIMEOUT.as_secs(),
                "spawn lock unavailable (contended past timeout or errored); proceeding without lock"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn acquire_then_release_allows_reacquire() {
        // Distinct command per test avoids cross-test contention on the shared
        // temp lock directory.
        let mut lock = super::open("labby-spawn-lock-test-bin", &["--unit-test".to_string()])
            .expect("open lock file");

        {
            let guard = super::acquire(Some(&mut lock)).await;
            assert!(guard.is_some(), "first acquire should succeed");
        } // guard dropped here, releasing the flock

        let again = super::acquire(Some(&mut lock)).await;
        assert!(again.is_some(), "reacquire after release should succeed");
    }

    #[tokio::test]
    async fn distinct_commands_open_distinct_locks() {
        let mut a = super::open("labby-spawn-lock-test-a", &[]).expect("open a");
        let mut b = super::open("labby-spawn-lock-test-b", &[]).expect("open b");
        let ga = super::acquire(Some(&mut a)).await;
        let gb = super::acquire(Some(&mut b)).await;
        assert!(
            ga.is_some() && gb.is_some(),
            "different commands hash to different lock files and must not block each other"
        );
    }

    #[tokio::test]
    async fn absent_lock_yields_no_guard() {
        let guard = super::acquire(None).await;
        assert!(guard.is_none(), "None input must proceed without a guard");
    }
}
