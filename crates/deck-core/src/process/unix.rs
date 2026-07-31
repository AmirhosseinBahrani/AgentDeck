//! Unix process groups, shared by macOS and Linux.
//!
//! Uses `nix` rather than raw `libc` so the signalling paths contain no `unsafe`.

use super::{ProcessGroup, Reaped};
use nix::errno::Errno;
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use std::io;

/// Puts the child in a new process group whose id equals its pid, so signalling the group
/// reaches every descendant rather than just the immediate child.
pub(super) fn configure(cmd: &mut tokio::process::Command) {
    // `tokio::process::Command::process_group` is inherent on unix; no std trait needed.
    cmd.process_group(0);
}

#[derive(Debug)]
pub struct UnixProcessGroup {
    pgid: Pid,
}

impl UnixProcessGroup {
    pub(super) fn adopt(child: &tokio::process::Child) -> io::Result<Self> {
        let pid = child.id().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "child already reaped, no pid")
        })?;
        // `configure` ran before spawn, so the group id equals the child's pid.
        Ok(Self {
            pgid: Pid::from_raw(pid as i32),
        })
    }

    fn signal(&self, sig: Option<Signal>) -> io::Result<()> {
        match killpg(self.pgid, sig) {
            Ok(()) => Ok(()),
            // The group exiting between our decision and the signal is the expected race,
            // not a failure — the caller's goal (nothing left running) already holds.
            Err(Errno::ESRCH) => Ok(()),
            Err(e) => Err(io::Error::from(e)),
        }
    }
}

impl ProcessGroup for UnixProcessGroup {
    fn request_stop(&self) -> io::Result<()> {
        self.signal(Some(Signal::SIGTERM))
    }

    fn interrupt(&self) -> io::Result<()> {
        self.signal(Some(Signal::SIGINT))
    }

    fn kill_now(&self) -> io::Result<()> {
        self.signal(Some(Signal::SIGKILL))
    }

    fn is_alive(&self) -> bool {
        // A null signal performs the existence and permission checks without delivering
        // anything, which is the standard liveness probe.
        killpg(self.pgid, None).is_ok()
    }

    fn pid(&self) -> u32 {
        self.pgid.as_raw() as u32
    }
}

pub(super) fn reap_orphan_group(pgid: u32) -> io::Result<Reaped> {
    let group = Pid::from_raw(pgid as i32);
    // Probed first so the caller can distinguish "we killed something that survived the crash"
    // from "the record was simply stale" — the two mean very different things in a log a user
    // reads after an unexpected exit.
    if killpg(group, None).is_err() {
        return Ok(Reaped::AlreadyGone);
    }
    match killpg(group, Some(Signal::SIGKILL)) {
        Ok(()) => Ok(Reaped::Killed),
        Err(Errno::ESRCH) => Ok(Reaped::AlreadyGone),
        Err(e) => Err(io::Error::from(e)),
    }
}

pub(super) fn pid_is_alive(pid: u32) -> bool {
    // 0 is not a pid to `kill`: it means "every process in the caller's own group", which
    // always succeeds and would report a nonexistent owner as alive. Rows written before
    // ownership was recorded carry exactly that value, so without this guard their orphaned
    // agents would look owned and never be reaped.
    if pid == 0 {
        return false;
    }
    kill(Pid::from_raw(pid as i32), None).is_ok()
}
