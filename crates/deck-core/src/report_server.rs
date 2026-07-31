//! Receives worker reports from the per-session MCP servers.
//!
//! One listener per session. Sharing one socket across agents would make a report attributable to
//! the wrong task if a stale server outlived its parent, and attribution is what the whole
//! completion protocol rests on.

use crate::reporting::{check_socket_path, ReportAck, ReportEnvelope};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Decides what to do with a report. Implemented by the supervisor side; the listener itself has
/// no opinion, so an accept can never originate here.
#[async_trait::async_trait]
pub trait ReportSink: Send + Sync + 'static {
    async fn accept(&self, envelope: ReportEnvelope) -> ReportAck;
}

pub struct ReportServer {
    path: PathBuf,
    shutdown: tokio_util::sync::CancellationToken,
}

impl ReportServer {
    /// Binds the socket and serves until cancelled.
    ///
    /// Removes any stale socket first: a crashed app leaves the file behind, and `bind` would then
    /// fail for every subsequent session with a misleading "address in use".
    pub async fn bind(path: PathBuf, sink: Arc<dyn ReportSink>) -> std::io::Result<Self> {
        // Checked up front so an over-long path reports what is actually wrong, rather than the
        // opaque InvalidInput that bind() would return.
        check_socket_path(&path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;

        if path.exists() {
            let _ = tokio::fs::remove_file(&path).await;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let listener = UnixListener::bind(&path)?;
        let shutdown = tokio_util::sync::CancellationToken::new();

        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { continue };
                            let sink = sink.clone();
                            // One task per connection: a slow sink must not stop other agents
                            // from reporting.
                            tokio::spawn(async move { serve(stream, sink).await });
                        }
                    }
                }
            });
        }

        Ok(Self { path, shutdown })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ReportServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        // Best effort: leaving the file behind is harmless because bind() clears a stale one.
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn serve(stream: UnixStream, sink: Arc<dyn ReportSink>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let ack = match serde_json::from_str::<ReportEnvelope>(&line) {
            Ok(envelope) => sink.accept(envelope).await,
            // Always answer, even on garbage. Silence would leave the agent's tool call hanging
            // until its own timeout, which reads to the model as a broken tool.
            Err(e) => ReportAck::rejected(format!("malformed report: {e}")),
        };

        let Ok(payload) = serde_json::to_string(&ack) else {
            break;
        };
        if write
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
        if write.flush().await.is_err() {
            break;
        }
    }
}
