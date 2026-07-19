//! Process-group signalling for shell-tool cancellation.
//!
//! # Safety isolation
//!
//! This is the **only** module in `tau-coding` permitted to use `unsafe`, and
//! only because cancelling a spawned shell must kill the whole process group
//! (the `sh -c` child plus any grandchildren it forks), not just the direct
//! child. Rust's `std::process::Child::kill` only signals the immediate
//! process, leaving orphaned grandchildren running. Sending `SIGTERM` to the
//! negative of the pid (`killpg`) is the POSIX-correct way to terminate a
//! group. The crate root declares `#![deny(unsafe_code)]` (hard error for any
//! `unsafe` unless explicitly allowed); this module opts back in locally so
//! the unsafe surface stays tiny and auditable.

#![allow(unsafe_code)]

/// Send `SIGTERM` to the process group rooted at `pid`.
///
/// Passing a positive pid to `killpg` targets the group whose id equals the
/// pid (we spawn the shell with `process_group(0)`, making the child its own
/// group leader). Returns `true` if the signal was delivered.
pub fn kill_process_group(pid: u32) -> bool {
    // SAFETY: `libc::killpg` with a valid pid and a standard signal number is
    // a well-defined POSIX call. The pid comes from `Child::id()` of a live
    // process group we spawned; even if the child already exited, `killpg`
    // either succeeds or returns ESRCH, which we treat as "nothing to kill".
    let rc = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGTERM) };
    rc == 0
}
