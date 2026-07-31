//! Drives the real `SessionActor` against a stand-in `claude` binary that replays captured
//! NDJSON. This exercises spawning, duplex I/O, translation, exit classification and
//! force-kill without spending tokens, which is also what makes it safe to run in CI.

#![cfg(unix)]

use deck_core::bus::EventBus;
use deck_core::domain::event::{AgentEvent, ExitReason};
use deck_core::domain::ids::SessionId;
use deck_core::runtime::claude_code::actor::{spawn_session, SessionCmd, SpawnOptions};
use deck_core::runtime::claude_code::argv::SessionConfig;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// These tests each spawn real child processes. Run in parallel they contend badly enough
/// that a child can go multiple seconds without being scheduled, which makes any
/// timeout-sensitive assertion flaky for reasons unrelated to the code under test. Holding
/// this guard serializes them; it costs a few seconds and removes the flakiness entirely.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Writes an executable stand-in for `claude`. It ignores the argv AgentDeck passes, which
/// is fine — argv construction is covered by its own unit tests.
fn fake_cli(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-actor-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Collects events until `SessionExited`, or the timeout elapses.
async fn drain(
    mut rx: tokio::sync::broadcast::Receiver<deck_core::domain::event::EventEnvelope>,
) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(env)) => {
                let done = matches!(env.event, AgentEvent::SessionExited { .. });
                out.push(env.event);
                if done {
                    break;
                }
            }
            _ => break,
        }
    }
    out
}

#[tokio::test]
async fn replays_a_real_session_into_translated_events() {
    let _serial = SERIAL.lock().await;
    let dir = tempdir("replay");
    let fixture = fixture_path("probe1.ndjson");
    let cli = fake_cli(&dir, &format!("cat {}", fixture.display()));

    let (bus, _durable) = EventBus::new();
    let observer = bus.subscribe();

    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.clone()));
    opts.program = cli.to_string_lossy().into_owned();

    let (handle, join) = spawn_session(opts, bus).await.expect("spawn");
    let events = drain(observer).await;
    let reason = join.await.expect("actor join");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionReady { .. })),
        "the first system/init must surface as SessionReady, got {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { tool, .. } if tool == "Read")),
        "expected the fixture's Read tool call to be translated"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnComplete { .. })),
        "expected a per-turn result event"
    );
    assert_eq!(reason, ExitReason::Clean);
    assert!(!handle.is_alive());
}

#[tokio::test]
async fn no_stream_line_ever_produces_a_parse_diagnostic() {
    let _serial = SERIAL.lock().await;
    // A Diagnostic here would mean production traffic is hitting an unhandled shape, which
    // in a live session risks dropping real events.
    let dir = tempdir("clean");
    let fixture = fixture_path("probe4.ndjson");
    let cli = fake_cli(&dir, &format!("cat {}", fixture.display()));

    let (bus, _durable) = EventBus::new();
    let observer = bus.subscribe();
    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.clone()));
    opts.program = cli.to_string_lossy().into_owned();

    let (_handle, join) = spawn_session(opts, bus).await.expect("spawn");
    let events = drain(observer).await;
    let _ = join.await;

    let diagnostics: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Diagnostic { message } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert!(
        diagnostics.is_empty(),
        "captured CLI output produced diagnostics: {diagnostics:#?}"
    );
}

#[tokio::test]
async fn force_kill_terminates_a_wedged_agent_that_never_handshakes() {
    let _serial = SERIAL.lock().await;
    // The worst case for a cooperative stop: no init, no output, ignores stdin. This is
    // exactly why kill_now must not route through the actor.
    let dir = tempdir("wedged");
    let cli = fake_cli(&dir, "sleep 300");

    let (bus, _durable) = EventBus::new();
    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.clone()));
    opts.program = cli.to_string_lossy().into_owned();

    let (handle, join) = spawn_session(opts, bus).await.expect("spawn");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(handle.is_alive(), "fake agent should be running");

    handle.kill_now().expect("kill_now on a wedged agent");

    let reason = tokio::time::timeout(Duration::from_secs(5), join)
        .await
        .expect("actor must finish promptly after force kill")
        .expect("join");

    assert_eq!(
        reason,
        ExitReason::Killed,
        "a user-initiated kill must be distinguishable from a crash, so it does not \
         consume a retry attempt"
    );
}

#[tokio::test]
async fn startup_failure_names_the_likely_cause_instead_of_just_timing_out() {
    let _serial = SERIAL.lock().await;
    // Without classification, an unauthenticated CLI, an unsupported version and a genuine
    // hang all present identically as "the agent never started".
    let dir = tempdir("auth");
    let cli = fake_cli(
        &dir,
        "echo 'Invalid API key - please run /login' >&2\nsleep 60",
    );

    let (bus, _durable) = EventBus::new();
    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.clone()));
    opts.program = cli.to_string_lossy().into_owned();
    // Generous relative to what the fake CLI needs, but far below production's 20s.
    // These tests spawn real processes, so a sub-second budget is flaky under load.
    opts.startup_timeout = Duration::from_secs(2);

    let (handle, join) = spawn_session(opts, bus).await.expect("spawn");

    let reason = tokio::time::timeout(Duration::from_secs(10), join)
        .await
        .expect("startup timeout must fire")
        .expect("join");

    match reason {
        ExitReason::StartupFailed { detail } => {
            assert!(
                detail.contains("not authenticated") && detail.contains("Invalid API key"),
                "detail should name the cause and quote the CLI's own output, got: {detail}"
            );
        }
        other => panic!("expected StartupFailed, got {other:?}"),
    }

    handle.kill_now().ok();
}

#[tokio::test]
async fn a_session_that_handshakes_is_not_killed_by_the_startup_timer() {
    let _serial = SERIAL.lock().await;
    // The timeout must only be armed until the first init, or every long turn would be
    // treated as a failed startup.
    let dir = tempdir("healthy");
    // The child blocks on stdin rather than sleeping, so the *test* controls when the turn
    // happens. Machine load can then only delay this test, never fail it.
    let cli = fake_cli(
        &dir,
        "echo '{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"/tmp\",\"tools\":[]}'\n\
         IFS= read -r _line\n\
         echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}'",
    );

    let (bus, _durable) = EventBus::new();
    let mut observer = bus.subscribe();
    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.clone()));
    opts.program = cli.to_string_lossy().into_owned();
    let startup_window = Duration::from_secs(2);
    opts.startup_timeout = startup_window;

    let (handle, join) = spawn_session(opts, bus).await.expect("spawn");

    // Wait for the handshake with a budget far larger than the startup window needs.
    let seen = tokio::time::timeout(Duration::from_secs(10), async {
        let mut seen: Vec<String> = Vec::new();
        loop {
            match observer.recv().await {
                Ok(env) => {
                    let ready = matches!(env.event, AgentEvent::SessionReady { .. });
                    let exited = matches!(env.event, AgentEvent::SessionExited { .. });
                    seen.push(format!("{:?}", env.event));
                    if ready || exited {
                        return seen;
                    }
                }
                Err(e) => {
                    seen.push(format!("recv error: {e}"));
                    return seen;
                }
            }
        }
    })
    .await
    .expect("handshake or exit within budget");

    assert!(
        seen.last().is_some_and(|s| s.starts_with("SessionReady")),
        "fake agent should have handshaked; observed: {seen:#?}"
    );

    // Idle past the startup window. A still-armed timer would fire here.
    tokio::time::sleep(startup_window + Duration::from_millis(500)).await;

    handle
        .send(SessionCmd::SendText("go".into()))
        .await
        .expect("session must still accept input after the startup window");

    let reason = tokio::time::timeout(Duration::from_secs(10), join)
        .await
        .expect("session should end on its own")
        .expect("join");

    assert_eq!(
        reason,
        ExitReason::Clean,
        "the startup timer must disarm once init is seen, otherwise every long turn would \
         be misreported as a failed startup"
    );
}

#[tokio::test]
async fn input_written_before_exit_reaches_the_child() {
    let _serial = SERIAL.lock().await;
    let dir = tempdir("stdin");
    let out = dir.join("received.txt");
    // `read` is a shell builtin and unbuffered, so the line lands on disk as soon as it is
    // received. A backgrounded `cat` would buffer and make this test timing-dependent.
    let cli = fake_cli(
        &dir,
        &format!(
            "echo '{{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"/tmp\",\"tools\":[]}}'\n\
             IFS= read -r line\n\
             printf '%s\\n' \"$line\" > {}\n\
             echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}'",
            out.display()
        ),
    );

    let (bus, _durable) = EventBus::new();
    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.clone()));
    opts.program = cli.to_string_lossy().into_owned();

    let (handle, join) = spawn_session(opts, bus).await.expect("spawn");
    handle
        .send(SessionCmd::SendText("hello agent".into()))
        .await
        .expect("send");

    // Let the child exit on its own so its write is complete before we read the file.
    let _ = tokio::time::timeout(Duration::from_secs(10), join).await;

    let written = std::fs::read_to_string(&out).unwrap_or_default();
    assert!(
        written.contains("hello agent"),
        "user text must be framed onto the child's stdin as NDJSON, got {written:?}"
    );
    assert!(
        written.contains("\"type\":\"user\""),
        "input must use the stream-json user envelope, got {written:?}"
    );
}
