//! The event-driven run loop.
//!
//! The loop reacts rather than polls. Agent activity wakes it; a bare tick only runs the free
//! sweep. That is what makes an idle run cost nothing: at a 2-second tick, a loop that iterated
//! unconditionally would spend most of its money discovering that nothing had happened.
//!
//! The tick still exists, because some conditions are time-based — a stalled agent, an expired
//! budget — and nothing will emit an event to announce them.

use crate::driver::{Driver, IterationOutcome, Run};
use crate::loop_engine::{RunPhase, Trigger};
use deck_core::domain::event::{AgentEvent, ExitReason};
use std::time::Duration;
use tokio::sync::mpsc;

/// How often the free sweep runs when nothing is happening.
pub const TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Maps a runtime event to a loop trigger.
///
/// Pure, so the mapping is testable without a bus. Returning `None` matters as much as returning
/// a trigger: most events — token deltas, individual tool calls — say nothing about whether the
/// supervisor has a decision to make, and waking on them would make the loop as expensive as
/// polling.
pub fn trigger_for(event: &AgentEvent) -> Option<Trigger> {
    match event {
        // A worker reported progress or a blocker: there may be something to decide.
        AgentEvent::ToolResult { .. } => None,
        AgentEvent::SessionExited { reason } => match reason {
            // A user-initiated kill is handled by the cancel path, not by re-planning.
            ExitReason::Killed => None,
            _ => Some(Trigger::SessionExited),
        },
        AgentEvent::PermissionResolved { .. } => Some(Trigger::HumanAnswered),
        AgentEvent::TurnComplete { .. } => Some(Trigger::ReportReceived),
        // Everything else is transcript detail, not a supervisory signal.
        _ => None,
    }
}

/// Why the loop stopped. Distinguishing these is what lets the UI say something useful rather
/// than just "finished".
#[derive(Debug, Clone, PartialEq)]
pub enum LoopExit {
    Terminal(RunPhase),
    Cancelled,
    /// Every trigger source closed, so nothing can wake the loop again.
    ChannelClosed,
}

/// Owns a run and drives it until it terminates.
type Observer<'a> = Box<dyn FnMut(&Run) + Send + 'a>;

pub struct RunLoop<'a> {
    triggers: mpsc::Receiver<Trigger>,
    tick: Duration,
    /// Called after each iteration so a UI can reflect what the supervisor just decided. Pushed
    /// rather than polled: polling would show a picture from mid-iteration.
    observer: Option<Observer<'a>>,
}

impl<'a> RunLoop<'a> {
    pub fn new(triggers: mpsc::Receiver<Trigger>) -> Self {
        Self {
            triggers,
            tick: TICK_INTERVAL,
            observer: None,
        }
    }

    pub fn observing(mut self, observer: impl FnMut(&Run) + Send + 'a) -> Self {
        self.observer = Some(Box::new(observer));
        self
    }

    /// Shorter ticks for tests, which must not wait out the production interval.
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Runs until the run terminates, is cancelled, or nothing can wake it again.
    pub async fn run(mut self, driver: &Driver<'_>, run: &mut Run) -> LoopExit {
        macro_rules! observe {
            () => {
                if let Some(observer) = self.observer.as_mut() {
                    observer(run);
                }
            };
        }
        // Start dirty: a fresh run has an empty graph and needs planning immediately rather than
        // waiting for the first tick.
        let mut dirty = true;
        let mut ticker = tokio::time::interval(self.tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; consume it so the initial iteration is driven by the
        // dirty flag rather than by an artificial tick.
        ticker.tick().await;

        loop {
            let outcome = driver.step(run, dirty).await;
            observe!();

            match outcome {
                IterationOutcome::Terminal(phase) => return LoopExit::Terminal(phase),
                IterationOutcome::Advanced { .. } => {
                    // Work was done. Anything further depends on the agents, so wait for them
                    // rather than immediately iterating again.
                    dirty = false;
                }
                IterationOutcome::Idle => {
                    dirty = false;
                }
            }

            tokio::select! {
                received = self.triggers.recv() => {
                    match received {
                        Some(Trigger::CancelRequested) => return LoopExit::Cancelled,
                        Some(trigger) => {
                            // Coalesce: several agents reporting at once should cause one
                            // iteration, not one each.
                            dirty = dirty || trigger.marks_dirty();
                            while let Ok(extra) = self.triggers.try_recv() {
                                if extra == Trigger::CancelRequested {
                                    return LoopExit::Cancelled;
                                }
                                dirty = dirty || extra.marks_dirty();
                            }
                        }
                        None => return LoopExit::ChannelClosed,
                    }
                }
                _ = ticker.tick() => {
                    // Left un-dirty on purpose: the sweep is free and decides for itself whether
                    // anything is worth doing.
                }
            }
        }
    }
}

/// Forwards bus events into a trigger channel.
///
/// Uses `try_send` so a full channel drops a redundant wake-up rather than backpressuring the
/// event bus. Losing a trigger is safe — the tick will pick the work up — whereas stalling the
/// bus would slow every agent.
pub fn forward_event(sender: &mpsc::Sender<Trigger>, event: &AgentEvent) -> bool {
    match trigger_for(event) {
        Some(trigger) => sender.try_send(trigger).is_ok(),
        None => false,
    }
}
