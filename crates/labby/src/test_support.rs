//! Shared test utilities for the `lab` binary crate.
//!
//! Only compiled in `#[cfg(test)]` builds. Import via `use crate::test_support::*;`.

use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

/// An in-memory writer for capturing tracing output in tests.
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedWriter(self.0.clone())
    }
}

pub struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Return the captured log output as a UTF-8 string.
pub fn captured_logs(buf: &SharedBuf) -> String {
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
}

/// A static mutex that serializes tests using `tracing::subscriber::set_default`.
///
/// `set_default` sets a thread-local subscriber override. When multiple tests run
/// in parallel and two tests end up on the same OS thread at different scheduler
/// windows, the thread-local stack can be left in an unexpected state. Holding this
/// lock for the duration of any test that calls `set_default` prevents that race.
///
/// Usage:
/// ```ignore
/// let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().unwrap();
/// ```
pub static TRACING_TEST_LOCK: Mutex<()> = Mutex::new(());
