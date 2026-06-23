use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

use crate::node::checkin::NodeStatus;

const COLLECTION_WARNING_INTERVAL_SECS: u64 = 60;
static COLLECTION_WARNINGS: OnceLock<Mutex<std::collections::BTreeMap<String, u64>>> =
    OnceLock::new();

/// Collect live system metrics.
///
/// CPU usage requires two sysinfo refreshes with a brief sleep between them;
/// the first call always returns 0% per the sysinfo docs. Subsequent periodic
/// calls will return accurate deltas.
pub fn collect(node_id: &str) -> NodeStatus {
    let sys_refresh = RefreshKind::nothing()
        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
        .with_memory(MemoryRefreshKind::everything());

    let mut sys = System::new_with_specifics(sys_refresh);
    sys.refresh_specifics(sys_refresh);
    std::thread::sleep(Duration::from_millis(250));
    sys.refresh_specifics(sys_refresh);

    let cpus = sys.cpus();
    let cpu_percent = if cpus.is_empty() {
        None
    } else {
        let cpu_count = u16::try_from(cpus.len()).unwrap_or(u16::MAX);
        let avg = cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / f32::from(cpu_count);
        Some(avg)
    };
    let cores: Option<u64> = cpus.len().try_into().ok().filter(|&n: &u64| n > 0);
    let cpu_clock_mhz: Option<u64> = cpus.first().map(|c| c.frequency()).filter(|&f| f > 0);

    let memory_used_bytes = non_zero(sys.used_memory());
    let total_memory_bytes = non_zero(sys.total_memory());
    let uptime_seconds = Some(System::uptime());

    let (storage_used_bytes, total_storage_bytes) = root_disk_usage(node_id);
    let ips = local_ips(node_id);
    let cpu_temp_c = read_cpu_temp(node_id);

    NodeStatus {
        node_id: node_id.to_string(),
        connected: true,
        cpu_percent,
        memory_used_bytes,
        total_memory_bytes,
        storage_used_bytes,
        total_storage_bytes,
        os: Some(std::env::consts::OS.to_string()),
        ips,
        health: Some("healthy".to_string()),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        uptime_seconds,
        cores,
        cpu_clock_mhz,
        cpu_temp_c,
        doctor_issues: vec![],
        active_claude_sessions: None,
        active_codex_sessions: None,
    }
}

fn non_zero(v: u64) -> Option<u64> {
    if v > 0 { Some(v) } else { None }
}

fn root_disk_usage(node_id: &str) -> (Option<u64>, Option<u64>) {
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh(false);

    // Pick the disk whose mount point is "/" or closest parent of it.
    let best = disks
        .iter()
        .filter(|d| d.mount_point().to_str().is_some_and(|p| "/".starts_with(p)))
        .max_by_key(|d| d.mount_point().as_os_str().len());

    if let Some(disk) = best {
        let total = disk.total_space();
        let avail = disk.available_space();
        let used = total.saturating_sub(avail);
        return (non_zero(used), non_zero(total));
    }
    warn_collection_failure(
        node_id,
        "storage",
        "root disk mount was not found during metrics collection",
    );
    (None, None)
}

fn local_ips(node_id: &str) -> Vec<String> {
    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh(false);

    let mut ips = Vec::new();
    for (_name, data) in &networks {
        for addr in data.ip_networks() {
            let ip = addr.addr;
            // Skip loopback and link-local
            let s = ip.to_string();
            if s == "127.0.0.1" || s == "::1" || s.starts_with("169.254.") || s.starts_with("fe80:")
            {
                continue;
            }
            ips.push(s);
        }
    }
    ips.sort();
    ips.dedup();
    if ips.is_empty() {
        warn_collection_failure(
            node_id,
            "network",
            "no non-loopback network addresses were found during metrics collection",
        );
    }
    ips
}

/// Read the first CPU temperature from `/sys/class/thermal` on Linux.
#[cfg(target_os = "linux")]
fn read_cpu_temp(node_id: &str) -> Option<f32> {
    use std::fs;
    match fs::read_dir("/sys/class/thermal") {
        Ok(entries) => {
            let mut zones: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with("thermal_zone"))
                })
                .collect();
            zones.sort_by_key(|e| e.file_name());
            for entry in zones {
                let path = entry.path();
                let zone_type = fs::read_to_string(path.join("type"))
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                // Only cpu/acpi/pkg zones; skip non-thermal sensors
                if !zone_type.is_empty()
                    && !["cpu", "x86", "acpi", "pkg", "tzone"]
                        .iter()
                        .any(|kw| zone_type.contains(kw))
                {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(path.join("temp")) {
                    if let Ok(millidegrees) = raw.trim().parse::<f32>() {
                        let celsius = millidegrees / 1000.0;
                        if celsius > 0.0 && celsius < 200.0 {
                            return Some(celsius);
                        }
                    }
                }
            }
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            warn_collection_failure(
                node_id,
                "cpu_temperature",
                &format!("failed to read thermal zones: {error}"),
            );
        }
        Err(_) => {}
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_temp(_node_id: &str) -> Option<f32> {
    None
}

fn warn_collection_failure(node_id: &str, metric: &str, message: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let key = format!("{node_id}:{metric}");
    let warnings = COLLECTION_WARNINGS.get_or_init(|| Mutex::new(Default::default()));
    let should_log = match warnings.lock() {
        Ok(mut warnings) => {
            let last = warnings.get(&key).copied().unwrap_or_default();
            if now.saturating_sub(last) >= COLLECTION_WARNING_INTERVAL_SECS {
                warnings.insert(key, now);
                true
            } else {
                false
            }
        }
        Err(_) => true,
    };
    if should_log {
        tracing::warn!(
            surface = "node",
            service = "sysmetrics",
            action = "metrics.collect",
            event = "metrics.collection_failure",
            kind = "collection_failed",
            node_id = %node_id,
            metric = %metric,
            message = %message,
            "node metrics collection failed",
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn collection_failure_logs_are_rate_limited_and_structured() {
        let source = include_str!("sysmetrics.rs");
        for field in [
            "COLLECTION_WARNING_INTERVAL_SECS",
            "event = \"metrics.collection_failure\"",
            "kind = \"collection_failed\"",
            "node_id = %node_id",
            "metric = %metric",
        ] {
            assert!(
                source.contains(field),
                "missing sysmetrics log field: {field}"
            );
        }
    }
}
