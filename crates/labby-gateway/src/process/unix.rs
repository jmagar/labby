#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::{Signal, kill as unix_kill, killpg};
#[cfg(unix)]
use nix::unistd::Pid;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(unix)]
pub fn send_signal(pid: u32, signal: Option<Signal>) -> nix::Result<()> {
    if pid == 0 {
        return Err(Errno::EINVAL);
    }

    let Ok(raw_pid) = i32::try_from(pid) else {
        return Err(Errno::EINVAL);
    };

    unix_kill(Pid::from_raw(raw_pid), signal)
}

#[cfg(unix)]
pub fn send_signal_process_group(pgid: u32, signal: Signal) -> nix::Result<()> {
    if pgid == 0 {
        return Err(Errno::EINVAL);
    }

    let Ok(raw_pgid) = i32::try_from(pgid) else {
        return Err(Errno::EINVAL);
    };

    killpg(Pid::from_raw(raw_pgid), signal)
}

#[cfg(unix)]
pub fn terminate_sigkill(pid: u32) -> nix::Result<()> {
    send_signal(pid, Some(Signal::SIGKILL))
}

#[cfg(unix)]
pub fn terminate_sigterm(pid: u32) -> nix::Result<()> {
    send_signal(pid, Some(Signal::SIGTERM))
}

#[cfg(unix)]
pub fn terminate_process_group_sigkill(pgid: u32) -> nix::Result<()> {
    send_signal_process_group(pgid, Signal::SIGKILL)
}

#[cfg(unix)]
pub fn terminate_process_group_sigterm(pgid: u32) -> nix::Result<()> {
    send_signal_process_group(pgid, Signal::SIGTERM)
}

#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    matches!(send_signal(pid, None), Ok(()) | Err(Errno::EPERM))
}

#[cfg(target_os = "linux")]
pub fn read_cmdline(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.is_empty() {
        return None;
    }
    let parts: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|segment| !segment.is_empty())
        .map(|segment| String::from_utf8_lossy(segment).into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(target_os = "linux")]
pub fn process_group_id(pid: u32) -> Option<u32> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = raw.rsplit_once(") ")?.1;
    after_comm.split_whitespace().nth(2)?.parse().ok()
}

#[cfg(target_os = "linux")]
pub fn process_has_ancestor(mut pid: u32, ancestor: u32) -> bool {
    while pid > 1 {
        if pid == ancestor {
            return true;
        }
        let raw = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(raw) => raw,
            Err(_) => return false,
        };
        let Some(after_comm) = raw.rsplit_once(") ").map(|(_, value)| value) else {
            return false;
        };
        let Some(parent) = after_comm
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return false;
        };
        if parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

#[cfg(target_os = "linux")]
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}
