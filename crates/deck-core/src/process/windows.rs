//! Windows process-tree control via Job Objects.
//!
//! Not implemented yet — v0.1 ships macOS only. This is a hard compile error rather than a
//! stub that returns `Ok(())`, because a silently-succeeding `kill_now` would leak agent
//! process trees and look like a working build.
//!
//! When implemented: `CreateJobObject` + `SetInformationJobObject` with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `AssignProcessToJobObject` at spawn, and
//! `TerminateJobObject` for `kill_now`. That is a stronger guarantee than Unix pgid, since
//! a child cannot detach itself from the job. There are no signals, so `request_stop` and
//! `interrupt` must rely on the in-band `control_request{interrupt}` — already step one of
//! the shutdown ladder, and portable.

compile_error!(
    "Windows support is not implemented for v0.1. Implement ProcessGroup with Job Objects \
     (see the module docs) before enabling a Windows target; a no-op would leak process trees."
);
