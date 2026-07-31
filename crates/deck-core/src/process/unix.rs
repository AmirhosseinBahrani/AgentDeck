//! Unix process groups, shared by macOS and Linux.
//!
//! Uses `nix` rather than raw `libc` so the signalling paths contain no `unsafe`.

use super::ProcessGroup;
use nix::errno::Errno;
use nix::sys::signal::{killpg, Signal};
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
