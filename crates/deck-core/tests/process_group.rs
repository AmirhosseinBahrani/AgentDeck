//! Proves that killing a session takes its whole descendant tree with it.
//!
//! This is the test that matters most in the process layer: `claude` spawns bash, node MCP
//! servers and LSPs, so a kill that only reaps the direct child leaves orphans holding
//! worktrees and burning tokens. A unit test on the signal call alone would pass while the
//! real behaviour was broken, so these spawn actual process trees.

#![cfg(unix)]

use deck_core::process::{self, ProcessGroup};
use std::time::Duration;
use tokio::process::Command;

/// Spawning real process trees in parallel contends badly on small CI runners, which makes
/// liveness assertions flaky for reasons unrelated to the code. Serialize them.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Spawns a shell that starts a long-lived grandchild, then waits. Mirrors the shape of
/// `claude` spawning tool subprocesses.
async fn spawn_tree() -> (tokio::process::Child, process::PlatformProcessGroup) {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg("sleep 300 & echo $! > /dev/null; sleep 300")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    process::configure_group(&mut cmd);

    let child = cmd.spawn().expect("spawn tree");
    let group = process::adopt(&child).expect("adopt group");
    // Give the shell a moment to fork its grandchild.
    tokio::time::sleep(Duration::from_millis(250)).await;
    (child, group)
}

/// Lists every live process in the given process group.
///
/// Enumerates all processes and filters by pgid rather than using `ps -g`: that flag selects
/// by process group on BSD but by *group name* on GNU/procps, so it silently returns nothing
/// on Linux and the test would assert against an empty set.
fn descendants_of(pgid: u32) -> Vec<u32> {
    let out = std::process::Command::new("ps")
        .args(["-e", "-o", "pid=,pgid="])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let pid = cols.next()?.parse::<u32>().ok()?;
            let group = cols.next()?.parse::<u32>().ok()?;
            (group == pgid).then_some(pid)
        })
        .collect()
}

#[tokio::test]
async fn kill_now_terminates_the_entire_process_group() {
    let _serial = SERIAL.lock().await;
    let (mut child, group) = spawn_tree().await;
    let pgid = group.pid();

    let before = descendants_of(pgid);
    assert!(
        before.len() >= 2,
        "expected a shell plus a grandchild in the group, saw {before:?} — \
         the test cannot prove tree-kill without a tree"
    );

    group.kill_now().expect("kill_now");
    let _ = child.wait().await;

    // Reaping is not instantaneous; poll briefly rather than assuming.
    let mut remaining = descendants_of(pgid);
    for _ in 0..40 {
        if remaining.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        remaining = descendants_of(pgid);
    }

    assert!(
        remaining.is_empty(),
        "force kill leaked descendants {remaining:?}; orphaned agents would keep \
         holding worktrees and consuming rate limit"
    );
}

#[tokio::test]
async fn kill_now_succeeds_even_when_the_group_is_already_gone() {
    let _serial = SERIAL.lock().await;
    // The actor may race a natural exit. Killing an already-dead group is the caller's
    // goal already being satisfied, so it must not surface as an error.
    let (mut child, group) = spawn_tree().await;
    group.kill_now().expect("first kill");
    let _ = child.wait().await;

    for _ in 0..40 {
        if !group.is_alive() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    group
        .kill_now()
        .expect("killing an already-dead group must be a no-op, not an error");
}

#[tokio::test]
async fn is_alive_tracks_the_group_lifecycle() {
    let _serial = SERIAL.lock().await;
    let (mut child, group) = spawn_tree().await;
    assert!(group.is_alive(), "group should be alive right after spawn");

    group.kill_now().expect("kill");
    let _ = child.wait().await;

    let mut alive = group.is_alive();
    for _ in 0..40 {
        if !alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        alive = group.is_alive();
    }
    assert!(!alive, "group should report dead after kill_now");
}

#[tokio::test]
async fn child_is_in_its_own_group_not_the_test_runners() {
    let _serial = SERIAL.lock().await;
    // If `process_group(0)` were ever dropped from the spawn path, the child would inherit
    // the test runner's group and `kill_now` would signal cargo itself.
    let (mut child, group) = spawn_tree().await;
    let own_pgid = nix::unistd::getpgid(None).expect("own pgid").as_raw() as u32;

    assert_ne!(
        group.pid(),
        own_pgid,
        "agent must not share a process group with its parent, or force kill would \
         terminate AgentDeck itself"
    );

    group.kill_now().ok();
    let _ = child.wait().await;
}
