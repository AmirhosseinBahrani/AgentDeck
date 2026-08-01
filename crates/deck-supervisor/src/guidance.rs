//! Telling the supervisor what you want, without becoming its prompt.
//!
//! The obvious way to let someone talk to the supervisor is a chat session. That would undo the
//! thing the controller design exists for: the supervisor has no conversation precisely so that
//! state cannot accumulate in a prompt across a run, and every decision stays a one-shot call
//! that can be replayed from the database.
//!
//! So guidance is an *input to the next decision*, not a message to a correspondent. A note is
//! stored, shown in the log as a decision you made, and folded into the code-assembled prompt
//! the next time the supervisor plans or assigns. The call is still one-shot and still
//! schema-validated; the note is just part of what code decided to tell it.
//!
//! What that buys, and its limits, are worth being precise about. Guidance can change how work
//! is shaped — smaller tasks, a preferred test command, which role should own something. It
//! cannot widen permissions, skip the verification gate, or mark work done, because none of
//! those read from the planner's prompt at all. They are enforced in code on the other side of
//! it. A note asking to "skip the tests" reaches the model and changes nothing, which is the
//! property that makes a free-text box safe here when it would not be anywhere else.

use serde::{Deserialize, Serialize};

/// Something the operator told the supervisor to take into account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guidance {
    pub id: String,
    pub text: String,
    /// Which iteration it arrived on, so the log shows when it started applying.
    pub given_at_iteration: u32,
    /// Whether the operator also asked for the plan to be redone.
    pub replan: bool,
}

impl Guidance {
    pub fn new(text: impl Into<String>, iteration: u32, replan: bool) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.into(),
            given_at_iteration: iteration,
            replan,
        }
    }
}

/// Renders standing guidance for a decision prompt.
///
/// Presented as constraints from the operator rather than as conversation, because that is what
/// it is — there is no thread here, and phrasing it as dialogue would invite the model to answer
/// rather than to comply.
pub fn render(notes: &[Guidance]) -> Option<String> {
    if notes.is_empty() {
        return None;
    }
    let mut out = String::from(
        "The operator has given the following standing instructions. Follow them unless they \
         conflict with the rules above, which take precedence:\n",
    );
    for note in notes {
        out.push_str("- ");
        out.push_str(note.text.trim());
        out.push('\n');
    }
    Some(out)
}

/// Guidance waiting to be applied.
///
/// Drained by the driver rather than pushed into the run, for the same reason every other
/// operator action is: it arrives whenever someone types, and mutating the run mid-stage would
/// change it underneath code already reading it.
pub trait GuidanceQueue: Send + Sync {
    fn drain(&self) -> Vec<Guidance>;
}

/// Says nothing. Used by tests and by runs with no operator attached.
pub struct NoGuidance;

impl GuidanceQueue for NoGuidance {
    fn drain(&self) -> Vec<Guidance> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_rendered_when_there_is_no_guidance() {
        // An empty section would still cost tokens on every planning call and tell the model
        // that instructions exist when none do.
        assert_eq!(render(&[]), None);
    }

    #[test]
    fn guidance_is_framed_as_subordinate_to_the_rules() {
        // The planner's own rules — objective_gate, executable criteria, no cycles — are what
        // keep a plan verifiable. An instruction must not be able to talk the model out of them.
        let rendered = render(&[Guidance::new("prefer small tasks", 0, false)]).unwrap();
        assert!(rendered.contains("take precedence"));
        assert!(rendered.contains("prefer small tasks"));
    }
}
