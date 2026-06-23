#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::{Signal, kill as unix_kill};
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
pub fn terminate_sigterm(pid: u32) -> nix::Result<()> {
    send_signal(pid, Some(Signal::SIGTERM))
}

#[cfg(target_os = "linux")]
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}
