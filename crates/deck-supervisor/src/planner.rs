//! The interface through which the supervisor consults a model.
//!
//! Every supervisor decision is a **one-shot** call: a fresh process, a code-assembled prompt, a
//! JSON schema, and a spend cap. There is deliberately no long-lived supervisor session — the
//! supervisor's memory is the database, not a conversation. A persistent session would accumulate
//! state nobody can inspect and would let earlier turns influence later decisions in ways no
//! validator could see.
//!
//! The trait exists so the loop can be driven by scripted answers in tests. Exercising the state
//! machine against a live model would be slow, non-deterministic, and would spend real rate limit
//! on assertions about control flow.

use crate::decision::{AssignmentChoice, FailureDisposition, ProposedPlan, ReviewResult};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq)]
pub struct ModelCall {
    /// Fully assembled by code. The model never sees raw conversation history.
    pub prompt: String,
    pub schema: serde_json::Value,
    /// Per-call ceiling, so one runaway decision cannot consume the run's budget.
    pub max_budget_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelResponse {
    pub structured: serde_json::Value,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlannerError {
    #[error("the model returned no structured output")]
    NoStructuredOutput,
    #[error("model call failed: {0}")]
    Failed(String),
    #[error("could not parse the model's response: {0}")]
    Parse(String),
}

/// One-shot, schema-validated model calls.
#[async_trait]
pub trait Planner: Send + Sync {
    async fn call(&self, call: ModelCall) -> Result<ModelResponse, PlannerError>;
}

/// Typed helpers over the raw call. Each returns `None` when the response is unusable, leaving the
/// caller to apply its deterministic fallback rather than retrying blindly.
pub struct Decisions<'a> {
    pub planner: &'a dyn Planner,
}

impl<'a> Decisions<'a> {
    pub fn new(planner: &'a dyn Planner) -> Self {
        Self { planner }
    }

    pub async fn plan(
        &self,
        prompt: String,
        schema: serde_json::Value,
        budget: f64,
    ) -> Result<(ProposedPlan, Option<f64>), PlannerError> {
        let response = self
            .planner
            .call(ModelCall {
                prompt,
                schema,
                max_budget_usd: budget,
            })
            .await?;
        let plan: ProposedPlan = serde_json::from_value(response.structured)
            .map_err(|e| PlannerError::Parse(e.to_string()))?;
        Ok((plan, response.cost_usd))
    }

    pub async fn assign(
        &self,
        prompt: String,
        schema: serde_json::Value,
        budget: f64,
    ) -> Result<(AssignmentChoice, Option<f64>), PlannerError> {
        let response = self
            .planner
            .call(ModelCall {
                prompt,
                schema,
                max_budget_usd: budget,
            })
            .await?;
        let choice: AssignmentChoice = serde_json::from_value(response.structured)
            .map_err(|e| PlannerError::Parse(e.to_string()))?;
        Ok((choice, response.cost_usd))
    }

    pub async fn review(
        &self,
        prompt: String,
        schema: serde_json::Value,
        budget: f64,
    ) -> Result<(ReviewResult, Option<f64>), PlannerError> {
        let response = self
            .planner
            .call(ModelCall {
                prompt,
                schema,
                max_budget_usd: budget,
            })
            .await?;
        let review: ReviewResult = serde_json::from_value(response.structured)
            .map_err(|e| PlannerError::Parse(e.to_string()))?;
        Ok((review, response.cost_usd))
    }

    pub async fn failure(
        &self,
        prompt: String,
        schema: serde_json::Value,
        budget: f64,
    ) -> Result<(FailureDisposition, Option<f64>), PlannerError> {
        let response = self
            .planner
            .call(ModelCall {
                prompt,
                schema,
                max_budget_usd: budget,
            })
            .await?;
        let disposition: FailureDisposition = serde_json::from_value(response.structured)
            .map_err(|e| PlannerError::Parse(e.to_string()))?;
        Ok((disposition, response.cost_usd))
    }
}

/// Records every call and replies from a script. Lets the loop's control flow be tested
/// deterministically and for free.
pub struct ScriptedPlanner {
    responses: parking_lot::Mutex<std::collections::VecDeque<Result<ModelResponse, String>>>,
    calls: parking_lot::Mutex<Vec<ModelCall>>,
}

impl ScriptedPlanner {
    pub fn new() -> Self {
        Self {
            responses: parking_lot::Mutex::new(Default::default()),
            calls: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, value: serde_json::Value, cost_usd: f64) -> &Self {
        self.responses.lock().push_back(Ok(ModelResponse {
            structured: value,
            cost_usd: Some(cost_usd),
        }));
        self
    }

    pub fn push_error(&self, message: &str) -> &Self {
        self.responses.lock().push_back(Err(message.to_string()));
        self
    }

    pub fn calls(&self) -> Vec<ModelCall> {
        self.calls.lock().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().len()
    }
}

impl Default for ScriptedPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Planner for ScriptedPlanner {
    async fn call(&self, call: ModelCall) -> Result<ModelResponse, PlannerError> {
        self.calls.lock().push(call);
        match self.responses.lock().pop_front() {
            Some(Ok(response)) => Ok(response),
            Some(Err(message)) => Err(PlannerError::Failed(message)),
            // Running past the script is a test bug, not a model failure, and should say so.
            None => Err(PlannerError::Failed(
                "ScriptedPlanner exhausted: the loop made more calls than the test scripted".into(),
            )),
        }
    }
}
