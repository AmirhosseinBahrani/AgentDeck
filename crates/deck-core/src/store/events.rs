//! The durable event log.
//!
//! Events arrive continuously from every agent, so the writer batches rather than committing one
//! transaction per event — at a few thousand events per second the per-transaction overhead, not
//! the disk, is what would fall behind.
//!
//! Reads come back through `since`, which is how the UI backfills after the lossy observer channel
//! drops something. That is the whole reason the log is durable: the broadcast path is allowed to
//! lose events precisely because this one cannot.

use crate::domain::event::EventEnvelope;
use crate::domain::ids::Seq;
use crate::store::{Store, StoreError};
use sqlx::Row;
use std::time::Duration;
use tokio::sync::mpsc;

/// Commit at whichever comes first. Both are deliberately small: a larger batch would amortise
/// better but widen the window in which a crash loses the tail.
pub const BATCH_SIZE: usize = 100;
pub const BATCH_INTERVAL: Duration = Duration::from_millis(20);

/// Drains the bus's durable channel into SQLite until it closes.
///
/// Runs forever, so it owns the receiver. If this task stopped, the bounded channel would fill and
/// backpressure the agents — which is the intended failure mode: slowing an agent beats losing its
/// audit log.
pub async fn run_writer(store: Store, mut incoming: mpsc::Receiver<EventEnvelope>) {
    let mut batch: Vec<EventEnvelope> = Vec::with_capacity(BATCH_SIZE);
    let mut ticker = tokio::time::interval(BATCH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            received = incoming.recv_many(&mut batch, BATCH_SIZE) => {
                if received == 0 {
                    // Channel closed: flush whatever is held rather than discarding it.
                    if !batch.is_empty() {
                        let _ = insert_batch(&store, &batch).await;
                    }
                    return;
                }
                if batch.len() >= BATCH_SIZE {
                    if let Err(e) = insert_batch(&store, &batch).await {
                        tracing::error!(%e, "failed to persist an event batch");
                    }
                    batch.clear();
                }
            }
            _ = ticker.tick() => {
                if !batch.is_empty() {
                    if let Err(e) = insert_batch(&store, &batch).await {
                        tracing::error!(%e, "failed to persist an event batch");
                    }
                    batch.clear();
                }
            }
        }
    }
}

/// One transaction per batch. Individually they would be ~100x slower, and a partially-written
/// batch would leave gaps in a log whose whole value is that it has none.
async fn insert_batch(store: &Store, batch: &[EventEnvelope]) -> Result<(), StoreError> {
    let mut tx = store.writer().begin().await?;

    for envelope in batch {
        let payload = serde_json::to_string(&envelope.event).unwrap_or_else(|_| "{}".into());
        // `kind` is denormalized out of the payload so the UI can filter without parsing JSON.
        let kind = event_kind(&envelope.event);

        sqlx::query(
            "INSERT INTO events (seq, at, kind, session_id, agent_id, task_id, payload_json)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(envelope.seq.0 as i64)
        .bind(envelope.at_ms)
        .bind(kind)
        .bind(envelope.session_id.map(|s| s.to_string()))
        .bind(envelope.agent_id.map(|a| a.to_string()))
        .bind(envelope.task_id.map(|t| t.to_string()))
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

fn event_kind(event: &crate::domain::event::AgentEvent) -> &'static str {
    use crate::domain::event::AgentEvent as E;
    match event {
        E::SessionReady { .. } => "session_ready",
        E::TurnStarted => "turn_started",
        E::Message { .. } => "message",
        E::ToolCall { .. } => "tool_call",
        E::ToolResult { .. } => "tool_result",
        E::PermissionRequest { .. } => "permission_request",
        E::PermissionResolved { .. } => "permission_resolved",
        E::RateLimited { .. } => "rate_limited",
        E::TurnComplete { .. } => "turn_complete",
        E::SessionExited { .. } => "session_exited",
        E::Unrecognized { .. } => "unrecognized",
        E::Diagnostic { .. } => "diagnostic",
    }
}

/// Events after `seq`, oldest first. The frontend's gap-backfill path.
pub async fn since(
    store: &Store,
    seq: Seq,
    limit: usize,
) -> Result<Vec<EventEnvelope>, StoreError> {
    let rows = sqlx::query(
        "SELECT seq, at, session_id, agent_id, task_id, payload_json
         FROM events WHERE seq > ? ORDER BY seq LIMIT ?",
    )
    .bind(seq.0 as i64)
    .bind(limit as i64)
    .fetch_all(store.reader())
    .await?;

    Ok(rows.iter().filter_map(row_to_envelope).collect())
}

/// A session's events, for restoring a transcript after a restart.
pub async fn for_session(
    store: &Store,
    session_id: &str,
    limit: usize,
) -> Result<Vec<EventEnvelope>, StoreError> {
    let rows = sqlx::query(
        "SELECT seq, at, session_id, agent_id, task_id, payload_json
         FROM events WHERE session_id = ? ORDER BY seq LIMIT ?",
    )
    .bind(session_id)
    .bind(limit as i64)
    .fetch_all(store.reader())
    .await?;

    Ok(rows.iter().filter_map(row_to_envelope).collect())
}

pub async fn count(store: &Store) -> Result<i64, StoreError> {
    let row = sqlx::query("SELECT COUNT(*) as n FROM events")
        .fetch_one(store.reader())
        .await?;
    Ok(row.get::<i64, _>("n"))
}

/// The highest seq on record, so a restart can resume numbering without colliding.
pub async fn max_seq(store: &Store) -> Result<Seq, StoreError> {
    let row = sqlx::query("SELECT COALESCE(MAX(seq), 0) as m FROM events")
        .fetch_one(store.reader())
        .await?;
    Ok(Seq(row.get::<i64, _>("m") as u64))
}

/// A row that will not deserialize is skipped rather than failing the whole read.
///
/// One unreadable event — written by an older build, say — must not make a session's entire
/// transcript unrecoverable.
fn row_to_envelope(row: &sqlx::sqlite::SqliteRow) -> Option<EventEnvelope> {
    let payload: String = row.try_get("payload_json").ok()?;
    let event = serde_json::from_str(&payload).ok()?;

    Some(EventEnvelope {
        seq: Seq(row.try_get::<i64, _>("seq").ok()? as u64),
        at_ms: row.try_get("at").ok()?,
        session_id: parse_id(row, "session_id"),
        agent_id: parse_id(row, "agent_id"),
        task_id: parse_id(row, "task_id"),
        event,
    })
}

fn parse_id<T: From<uuid::Uuid>>(row: &sqlx::sqlite::SqliteRow, column: &str) -> Option<T> {
    let raw: Option<String> = row.try_get(column).ok()?;
    raw.and_then(|s| uuid::Uuid::parse_str(&s).ok())
        .map(T::from)
}
