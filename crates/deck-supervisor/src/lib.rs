//! The supervisor is a deterministic controller around Claude, not a long-lived prompt.
//!
//! Its memory is the database: every decision is a one-shot `claude -p --json-schema`
//! call with a code-assembled input projection, and every model output passes a validation
//! ladder before it is allowed to mutate state. State transitions, retry counting,
//! dependency readiness, concurrency limits and completion detection are pure code.
//!
//! The loop lives in [`loop_engine`], the bounded decision points and their validation ladder in
//! [`decision`], the dependency graph in [`graph`], and contracts plus the deterministic
//! verification gate in [`contract`].

pub mod autonomy;
pub mod cli_planner;
pub mod contract;
pub mod decision;
pub mod driver;
pub mod escalation;
pub mod graph;
pub mod loop_engine;
pub mod planner;
pub mod run_loop;
pub mod workspaces;
