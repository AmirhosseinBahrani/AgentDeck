//! Process-tree control.
//!
//! `claude` spawns children of its own — bash, node MCP servers, LSPs — so killing the
//! parent PID leaks the rest. The mechanism for killing a whole tree differs completely
//! between platforms, which is why it lives behind a trait from the first commit rather
//! than being retrofitted later.

use std::io;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::UnixProcessGroup as PlatformProcessGroup;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::JobObjectProcessGroup as PlatformProcessGroup;

/// Owns the OS-level grouping that makes a whole process tree killable.
///
/// Implementations must make [`kill_now`](ProcessGroup::kill_now) safe to call from any
/// task at any time, including concurrently with a graceful shutdown already in progress.
pub trait ProcessGroup: Send + Sync + 'static {
    /// Ask the tree to stop and let it clean up. Unix sends SIGTERM; on Windows there is
    /// no signal equivalent, so callers must rely on the in-band interrupt first.
    fn request_stop(&self) -> io::Result<()>;

    /// Interrupt the foreground work without necessarily ending the process.
    fn interrupt(&self) -> io::Result<()>;

    /// Unconditional, immediate termination of every process in the group.
    ///
    /// This is the operation the UI's "Force kill" maps to. It must not await the child,
    /// consult the control channel, or depend on the session actor being responsive — a
    /// wedged actor is precisely the situation it exists for.
    fn kill_now(&self) -> io::Result<()>;

    /// True while at least one process in the group is alive.
    fn is_alive(&self) -> bool;

    fn pid(&self) -> u32;
}

/// Configures a `Command` so the spawned child becomes the root of its own killable group.
/// Must be called before spawn.
pub fn configure_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    unix::configure(cmd);
    #[cfg(windows)]
    windows::configure(cmd);
}

/// Wraps an already-spawned child in its platform process group.
pub fn adopt(child: &tokio::process::Child) -> io::Result<PlatformProcessGroup> {
    PlatformProcessGroup::adopt(child)
}

/// What became of a group we tried to reap after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaped {
    /// It was still running and has been killed.
    Killed,
    /// Nothing was there; only the record needed clearing.
    AlreadyGone,
}

/// Kills a process group recorded by a previous run of the app.
///
/// Adopted by group id rather than by handle, because the handle died with the app that
/// crashed. On Unix a process group outlives its creator, so agents genuinely keep running and
/// have to be killed explicitly. On Windows the Job Object is destroyed with the app and
/// `KILL_ON_JOB_CLOSE` has already taken the tree down, so there is nothing left to reap.
pub fn reap_orphan_group(pgid: u32) -> io::Result<Reaped> {
    #[cfg(unix)]
    {
        unix::reap_orphan_group(pgid)
    }
    #[cfg(windows)]
    {
        let _ = pgid;
        Ok(Reaped::AlreadyGone)
    }
}

/// True while the process is alive. Used to decide whether a recorded owner is still around.
pub fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unix::pid_is_alive(pid)
    }
    #[cfg(windows)]
    {
        let _ = pid;
        // Conservative: an owner we cannot probe is treated as live, so a second instance
        // never reaps the first instance's agents.
        true
    }
}
