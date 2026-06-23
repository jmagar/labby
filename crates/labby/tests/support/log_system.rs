use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fd_lock::RwLock;

pub(crate) fn test_lock() -> RwLock<File> {
    let lock_dir = std::env::temp_dir().join("lab-test-locks");
    fs::create_dir_all(&lock_dir).expect("create lab test lock dir");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_dir.join("log-system.lock"))
        .expect("open log system test lock");
    RwLock::new(lock_file)
}

pub(crate) struct InstalledLogSystemGuard;

impl InstalledLogSystemGuard {
    #[must_use]
    pub(crate) fn new() -> Self {
        labby::dispatch::logs::client::clear_installed_log_system_for_test();
        Self
    }
}

impl Drop for InstalledLogSystemGuard {
    fn drop(&mut self) {
        labby::dispatch::logs::client::clear_installed_log_system_for_test();
    }
}

#[allow(dead_code)]
pub(crate) struct SqlitePathCleanup {
    path: PathBuf,
}

#[allow(dead_code)]
impl SqlitePathCleanup {
    #[must_use]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for SqlitePathCleanup {
    fn drop(&mut self) {
        if let Err(error) = cleanup_sqlite_path(&self.path) {
            eprintln!("failed to clean up {}: {error}", self.path.display());
        }
    }
}

#[allow(dead_code)]
pub(crate) fn cleanup_sqlite_path(path: &Path) -> std::io::Result<()> {
    let with_suffix = |suffix: &str| -> PathBuf {
        let mut os = path.as_os_str().to_os_string();
        os.push(suffix);
        PathBuf::from(os)
    };
    let sidecars = [path.to_path_buf(), with_suffix("-wal"), with_suffix("-shm")];

    for candidate in sidecars {
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Ok(())
}
