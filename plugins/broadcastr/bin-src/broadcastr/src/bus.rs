use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Append one JSON line to a bus file, rotating if it exceeds the size limit.
pub fn append(config: &Config, bus: &Path, line: &str) -> io::Result<()> {
    if let Some(p) = bus.parent() {
        fs::create_dir_all(p)?;
    }
    rotate_if_needed(config, bus)?;
    let mut f = OpenOptions::new().create(true).append(true).open(bus)?;
    // Single write_all under O_APPEND: atomic for buffers < PIPE_BUF.
    f.write_all(format!("{line}\n").as_bytes())
}

fn rotate_if_needed(config: &Config, bus: &Path) -> io::Result<()> {
    let size = match fs::metadata(bus) {
        Ok(m) => m.len(),
        Err(_) => return Ok(()),
    };
    if size < config.bus_max_bytes {
        return Ok(());
    }

    // Try-lock: create exclusively; skip rotation if another process beat us.
    let lock = bus.with_extension("rotate.lock");
    let Ok(_lock_file) = OpenOptions::new().write(true).create_new(true).open(&lock) else {
        return Ok(());
    };

    // Re-check under lock (TOCTOU).
    if fs::metadata(bus).map(|m| m.len()).unwrap_or(0) < config.bus_max_bytes {
        let _ = fs::remove_file(&lock);
        return Ok(());
    }

    let base = bus.file_name().unwrap().to_string_lossy();
    let dir = bus.parent().unwrap();

    for i in (1..config.bus_retain).rev() {
        let from = dir.join(format!("{base}.{i}"));
        let to = dir.join(format!("{base}.{}", i + 1));
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    let _ = fs::rename(bus, dir.join(format!("{base}.1")));
    // Touch new bus without truncating (concurrent writers may already be there).
    let _ = OpenOptions::new().create(true).append(true).open(bus);
    let _ = fs::remove_file(&lock);
    Ok(())
}

/// Polls one or more bus files for new lines since the last call.
/// Tracks byte offsets per file; resets on rotation (file shrinks).
pub struct BusTailer {
    paths: Vec<PathBuf>,
    cursors: HashMap<PathBuf, u64>,
}

impl BusTailer {
    /// Start tailing from the current EOF of each file (tail -n0 semantics).
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let cursors = paths
            .iter()
            .map(|p| (p.clone(), fs::metadata(p).map(|m| m.len()).unwrap_or(0)))
            .collect();
        Self { paths, cursors }
    }

    /// Read any new complete lines. Non-blocking; returns empty vec if nothing new.
    pub fn poll(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for path in &self.paths {
            let cursor = self.cursors.entry(path.clone()).or_insert(0);
            let file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);

            // Rotation: file shrank → start over from the new file's beginning.
            if file_size < *cursor {
                *cursor = 0;
            }
            if file_size == *cursor {
                continue;
            }

            let Ok(mut file) = File::open(path) else { continue };
            if file.seek(SeekFrom::Start(*cursor)).is_err() {
                continue;
            }

            let mut reader = BufReader::new(file);
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        *cursor += n as u64;
                        let line = buf.trim_end_matches(['\n', '\r']);
                        if !line.is_empty() {
                            out.push(line.to_string());
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        out
    }
}
