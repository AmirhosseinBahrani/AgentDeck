//! Worker reporting, end to end through the real MCP binary.
//!
//! Drives the compiled `deck-mcp` over actual stdio and an actual unix socket, because the thing
//! worth proving is that a tool call reaches the app and that a rejection reaches the agent — and
//! a mocked transport would prove neither.
//!
//! These use a multi-threaded runtime deliberately. The test blocks on the child's stdio, which on
//! a current-thread runtime would starve the socket listener and deadlock: the MCP server's
//! connect would never be accepted because the accepting task never gets to run.

#![cfg(unix)]

use deck_core::report_server::{ReportServer, ReportSink};
use deck_core::reporting::{
    mcp_config, socket_path, ReportAck, ReportEnvelope, WorkerReport, SOCKET_ENV, TASK_ENV,
};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

/// Records what arrived and answers according to a fixed verdict.
struct RecordingSink {
    received: parking_lot::Mutex<Vec<ReportEnvelope>>,
    accept: bool,
    message: String,
}

impl RecordingSink {
    fn new(accept: bool, message: &str) -> Arc<Self> {
        Arc::new(Self {
            received: parking_lot::Mutex::new(Vec::new()),
            accept,
            message: message.to_string(),
        })
    }

    fn received(&self) -> Vec<ReportEnvelope> {
        self.received.lock().clone()
    }
}

#[async_trait::async_trait]
impl ReportSink for RecordingSink {
    async fn accept(&self, envelope: ReportEnvelope) -> ReportAck {
        self.received.lock().push(envelope);
        if self.accept {
            ReportAck::accepted(&self.message)
        } else {
            ReportAck::rejected(&self.message)
        }
    }
}

fn mcp_binary() -> PathBuf {
    // Built by cargo alongside the tests; CARGO_BIN_EXE_ is only set for the owning crate, so
    // locate it relative to the test executable instead.
    let mut dir = std::env::current_exe().expect("test exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join("deck-mcp")
}

struct Server {
    child: Child,
    /// Kept for the process's lifetime. Building a fresh `BufReader` per call would discard
    /// whatever it had already buffered, silently losing a response.
    stdout: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn start(socket: &PathBuf, task_id: &str) -> Self {
        let mut child = Command::new(mcp_binary())
            .env(SOCKET_ENV, socket)
            .env(TASK_ENV, task_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn deck-mcp — run `cargo build` first");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self { child, stdout }
    }

    /// Sends a JSON-RPC request and reads the response.
    fn request(&mut self, value: serde_json::Value) -> serde_json::Value {
        let stdin = self.child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{value}").expect("write");
        stdin.flush().expect("flush");

        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read");
        serde_json::from_str(line.trim()).expect("response is JSON-RPC")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Deliberately short. Unix socket paths are capped near 104 bytes, and macOS temp dirs are
/// already long, so a descriptive directory name here would push past the limit.
fn tmpdir(_tag: &str) -> PathBuf {
    let short: String = uuid::Uuid::new_v4().to_string().chars().take(8).collect();
    let dir = PathBuf::from("/tmp").join(format!("adt-{short}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_advertises_the_three_reporting_tools() {
    let dir = tmpdir("list");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink).await.unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list"
    }));

    let names: Vec<String> = response["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect();

    assert_eq!(
        names,
        vec!["report_progress", "claim_task_done", "raise_blocker"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_claim_tool_tells_the_agent_that_stopping_without_it_is_a_failure() {
    // A worker that does not know this will simply stop when it thinks it is done, and be recorded
    // as having failed. The description is the only place it learns otherwise.
    let dir = tmpdir("desc");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink).await.unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list"
    }));

    let claim = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "claim_task_done")
        .expect("claim_task_done");
    let description = claim["description"].as_str().unwrap();

    assert!(description.contains("ONLY way to finish"));
    assert!(description.contains("recorded as failed"));
    assert!(
        description.contains("verified by the supervisor"),
        "the worker should know its claims are checked, not taken on trust"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claim_reaches_the_app_attributed_to_the_right_task() {
    // Misattribution would let one agent complete another's task.
    let dir = tmpdir("claim");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "recorded");
    let _server = ReportServer::bind(socket.clone(), sink.clone())
        .await
        .unwrap();

    let mut mcp = Server::start(&socket, "task-abc");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "claim_task_done", "arguments": { "summary": "endpoint added" } }
    }));

    assert!(
        response["result"]["isError"].is_null(),
        "should not be an error"
    );
    assert!(response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("recorded"));

    let received = sink.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].task_id, "task-abc");
    assert!(matches!(
        received[0].report,
        WorkerReport::ClaimTaskDone { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_claim_is_surfaced_to_the_agent_as_a_tool_error() {
    // A rejection presented as ordinary output would read as confirmation with an odd message, and
    // the agent would stop believing it had finished.
    let dir = tmpdir("reject");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(false, "verification failed: 2 tests are red");
    let _server = ReportServer::bind(socket.clone(), sink).await.unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "claim_task_done", "arguments": { "summary": "done" } }
    }));

    assert_eq!(response["result"]["isError"], serde_json::json!(true));
    assert!(response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("2 tests are red"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progress_and_blockers_carry_their_structured_fields() {
    let dir = tmpdir("progress");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink.clone())
        .await
        .unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {
            "name": "report_progress",
            "arguments": {
                "summary": "halfway",
                "completed_work": ["handler"],
                "remaining_work": ["tests"],
                "blockers": []
            }
        }
    }));
    mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "raise_blocker", "arguments": { "reason": "API contract unclear" } }
    }));

    let received = sink.received();
    assert_eq!(received.len(), 2);

    match &received[0].report {
        WorkerReport::ReportProgress {
            summary,
            completed_work,
            remaining_work,
            ..
        } => {
            assert_eq!(summary, "halfway");
            assert_eq!(completed_work, &vec!["handler".to_string()]);
            assert_eq!(remaining_work, &vec!["tests".to_string()]);
        }
        other => panic!("expected progress, got {other:?}"),
    }
    assert!(matches!(
        received[1].report,
        WorkerReport::RaiseBlocker { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_missing_a_required_field_is_refused_without_reaching_the_app() {
    // Validating here keeps malformed calls out of the supervisor's state entirely.
    let dir = tmpdir("missing");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink.clone())
        .await
        .unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 6, "method": "tools/call",
        "params": { "name": "claim_task_done", "arguments": {} }
    }));

    assert_eq!(response["result"]["isError"], serde_json::json!(true));
    assert!(sink.received().is_empty(), "nothing should reach the app");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_tool_is_refused_rather_than_forwarded() {
    let dir = tmpdir("unknown");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink.clone())
        .await
        .unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": "delete_everything", "arguments": {} }
    }));

    assert_eq!(response["result"]["isError"], serde_json::json!(true));
    assert!(sink.received().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_report_that_cannot_be_delivered_tells_the_agent_it_was_not_recorded() {
    // Silence would let a worker believe it had claimed completion when the app never heard, and
    // the task would fail for reasons the agent could not understand.
    let dir = tmpdir("nosocket");
    let socket = dir.join("nothing-listening.sock");

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 8, "method": "tools/call",
        "params": { "name": "claim_task_done", "arguments": { "summary": "done" } }
    }));

    assert_eq!(response["result"]["isError"], serde_json::json!(true));
    assert!(response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not recorded"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_initialize_handshake_advertises_tool_support() {
    let dir = tmpdir("init");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink).await.unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 9, "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {} }
    }));

    assert!(response["result"]["capabilities"]["tools"].is_object());
    assert_eq!(response["result"]["serverInfo"]["name"], "agentdeck");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notification_is_not_answered() {
    // Replying to a notification is a protocol violation and desynchronizes the client.
    let dir = tmpdir("notify");
    let socket = socket_path(&dir, "s1");
    let sink = RecordingSink::new(true, "ok");
    let _server = ReportServer::bind(socket.clone(), sink).await.unwrap();

    let mut mcp = Server::start(&socket, "task-1");
    {
        let stdin = mcp.child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        stdin.flush().unwrap();
    }

    // The next request must still line up, which it only does if nothing was emitted for the
    // notification.
    let response = mcp.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 10, "method": "ping"
    }));
    assert_eq!(response["id"], serde_json::json!(10));
}

#[test]
fn the_mcp_config_points_the_cli_at_the_server_with_its_session_socket() {
    let config = mcp_config(
        &PathBuf::from("/usr/local/bin/deck-mcp"),
        &PathBuf::from("/tmp/agentdeck-report-abc.sock"),
        "task-xyz",
    );

    let server = &config["mcpServers"]["agentdeck"];
    assert_eq!(server["command"], "/usr/local/bin/deck-mcp");
    assert_eq!(server["env"][SOCKET_ENV], "/tmp/agentdeck-report-abc.sock");
    assert_eq!(server["env"][TASK_ENV], "task-xyz");
}

#[test]
fn each_session_gets_its_own_socket() {
    // A shared socket would let a stale server outliving its parent report against another
    // agent's task.
    let dir = PathBuf::from("/tmp");
    assert_ne!(socket_path(&dir, "a"), socket_path(&dir, "b"));
}

#[test]
fn an_over_long_socket_path_is_reported_clearly() {
    // bind() would otherwise fail with an opaque InvalidInput, which is very hard to connect to
    // "your temp directory is too deep".
    let deep = PathBuf::from("/tmp").join("x".repeat(200));
    let err = deck_core::reporting::check_socket_path(&deep).expect_err("should be refused");
    assert!(err.length > deck_core::reporting::MAX_SOCKET_PATH);
    assert!(err.to_string().contains("shorter runtime directory"));
}

#[test]
fn generated_socket_paths_fit_within_the_limit() {
    let path = socket_path(&PathBuf::from("/tmp"), &uuid::Uuid::new_v4().to_string());
    assert!(
        deck_core::reporting::check_socket_path(&path).is_ok(),
        "{path:?}"
    );
}
