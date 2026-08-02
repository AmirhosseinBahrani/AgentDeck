//! How much the supervisor is allowed to do without being asked.
//!
//! This is not a UI preference with a backend that ignores it. Autonomy decides whether a real
//! process gets spawned into a real worktree with edit permissions, so it has to be enforced
//! where that decision is made — in the dispatch stage — and nowhere else can be allowed to
//! shortcut it. A mode that only greyed out a button would be a safety claim the code does not
//! make good on.
//!
//! The three modes differ along two axes and no others, which is what keeps them explicable:
//! whether a dispatch needs a human's approval, and whether a failure is retried or handed back.

use deck_core::domain::ids::TaskId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Plans and assigns, but starts nothing and retries nothing. Every dispatch waits for a
    /// human, and a failure comes straight back rather than being attempted again.
    Manual,
    /// Plans, assigns, starts agents and retries on its own. The operator is interrupted only
    /// when something genuinely needs them: a permission request, a merge conflict, a blocker.
    #[default]
    Assisted,
    /// Runs unattended. The operator is told what happened rather than asked first.
    Autonomous,
}

impl Autonomy {
    /// Whether starting an agent requires a human to say so first.
    ///
    /// Only Manual. Assisted used to gate every dispatch too, which meant the ordinary way to run
    /// the app was to sit clicking Start agent once per task while the supervisor waited — the
    /// approval carried no judgement, because the decision of whether a task should run at all was
    /// already made when it was planned and assigned. What genuinely needs a human is a permission
    /// request or a conflict, and those escalate on their own path regardless of mode.
    pub fn dispatch_needs_approval(self) -> bool {
        matches!(self, Autonomy::Manual)
    }

    /// Whether a failed task may be attempted again without asking.
    ///
    /// Manual says no: the point of the mode is that nothing happens twice without a human
    /// seeing it happen once, and a silent retry is exactly that.
    pub fn may_retry(self) -> bool {
        !matches!(self, Autonomy::Manual)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Autonomy::Manual => "manual",
            Autonomy::Assisted => "assisted",
            Autonomy::Autonomous => "autonomous",
        }
    }
}

/// Dispatch approvals a human has granted, waiting to be applied.
///
/// Drained by the driver rather than pushed into the run, for the same reason worker reports
/// are: an approval arrives whenever the operator clicks, and mutating the graph mid-stage would
/// change it underneath code already reading it.
pub trait ApprovalQueue: Send + Sync {
    fn drain(&self) -> Vec<TaskId>;
}

/// Grants nothing. Used by tests and by autonomous runs, which never consult it.
pub struct NoApprovals;

impl ApprovalQueue for NoApprovals {
    fn drain(&self) -> Vec<TaskId> {
        Vec::new()
    }
}

/// Where the driver reads the current mode from.
///
/// The mode used to be fixed in `RunConfig` at run start, which made the control in the UI
/// inert for the entire life of a run — and a run is exactly when someone decides they would
/// rather approve each agent, or stop being asked. Both read sites are consulted afresh at the
/// moment they matter (dispatch, and a failure), so nothing about the loop required it to be
/// constant; it simply had nowhere else to read from.
pub trait AutonomySource: Send + Sync {
    fn current(&self) -> Autonomy;
}

/// Holds whatever the run started with. Used by tests and by runs with no operator attached.
pub struct FixedAutonomy(pub Autonomy);

impl AutonomySource for FixedAutonomy {
    fn current(&self) -> Autonomy {
        self.0
    }
}
