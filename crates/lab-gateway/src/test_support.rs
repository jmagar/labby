//! Shared test utilities for the `lab-gateway` crate.
//!
//! Only compiled in `#[cfg(test)]` builds. Import via `use crate::test_support::*;`.
//! Vendored from `lab`'s `crate::test_support` so the moved upstream pool's
//! tracing-capture tests stay self-contained in this crate.

use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

/// An in-memory writer for capturing tracing output in tests.
#[derive(Clone, Default)]
pub(crate) struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedWriter(self.0.clone())
    }
}

pub(crate) struct SharedWriter(Arc<Mutex<Vec<u8>>>);

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
pub(crate) fn captured_logs(buf: &SharedBuf) -> String {
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
}

/// A static mutex that serializes tests using `tracing::subscriber::set_default`.
///
/// `set_default` sets a thread-local subscriber override. When multiple tests run
/// in parallel and two tests end up on the same OS thread at different scheduler
/// windows, the thread-local stack can be left in an unexpected state. Holding this
/// lock for the duration of any test that calls `set_default` prevents that race.
pub(crate) static TRACING_TEST_LOCK: Mutex<()> = Mutex::new(());
