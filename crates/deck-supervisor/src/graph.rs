//! Task dependency graph.
//!
//! The supervisor mutates this graph mid-run as it discovers follow-up work, so two properties
//! matter more than performance:
//!
//! 1. **Mutations are all-or-nothing.** A batch that would introduce a cycle is rejected
//!    entirely rather than half-applied, because a partially-applied plan is harder to reason
//!    about than a rejected one — and the planner can be asked again.
//! 2. **Deadlock is detectable.** If nothing is ready, running, in review, or waiting on a
//!    human, the run is stuck. Without an explicit check the loop would spin forever looking
//!    busy, which is the failure mode that wastes the most time before anyone notices.

use deck_core::domain::ids::TaskId;
use deck_core::domain::task::{TaskState, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Hard edges gate readiness. Soft edges only express preferred ordering, so a soft dependency
/// that fails does not strand its dependents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Hard,
    Soft,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    /// The task that must finish first.
    pub from: TaskId,
    /// The task that waits.
    pub to: TaskId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GraphError {
    #[error("edge would create a dependency cycle: {}", format_cycle(cycle))]
    Cycle { cycle: Vec<TaskId> },
    #[error("a task cannot depend on itself ({0})")]
    SelfDependency(TaskId),
    #[error("edge references unknown task {0}")]
    UnknownTask(TaskId),
}

fn format_cycle(cycle: &[TaskId]) -> String {
    cycle
        .iter()
        .map(|id| id.to_string()[..8].to_string())
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Additive-biased mutation set. There is no "remove edge": the supervisor cancels tasks
/// explicitly rather than quietly rewriting history, so a replan cannot erase the reason a
/// dependency existed.
#[derive(Debug, Clone, Default)]
pub struct Mutation {
    pub add_tasks: Vec<TaskState>,
    pub add_edges: Vec<Edge>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskGraph {
    tasks: HashMap<TaskId, TaskState>,
    edges: Vec<Edge>,
}

impl TaskGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn get(&self, id: TaskId) -> Option<&TaskState> {
        self.tasks.get(&id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &TaskState> {
        self.tasks.values()
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Replaces a task's state. The state machine in `deck-core` owns transitions; the graph
    /// only stores the outcome.
    pub fn set_state(&mut self, state: TaskState) {
        self.tasks.insert(state.id, state);
    }

    /// Applies a mutation batch, or rejects all of it.
    ///
    /// Validation runs against the *post-mutation* adjacency, since a batch can introduce a
    /// cycle that neither the before nor the after state shows in isolation.
    pub fn apply(&mut self, mutation: Mutation) -> Result<(), GraphError> {
        let mut candidate_tasks = self.tasks.clone();
        for task in &mutation.add_tasks {
            candidate_tasks.insert(task.id, task.clone());
        }

        let mut candidate_edges = self.edges.clone();
        for edge in &mutation.add_edges {
            if edge.from == edge.to {
                return Err(GraphError::SelfDependency(edge.from));
            }
            if !candidate_tasks.contains_key(&edge.from) {
                return Err(GraphError::UnknownTask(edge.from));
            }
            if !candidate_tasks.contains_key(&edge.to) {
                return Err(GraphError::UnknownTask(edge.to));
            }
            if !candidate_edges.contains(edge) {
                candidate_edges.push(edge.clone());
            }
        }

        if let Some(cycle) = find_cycle(&candidate_tasks, &candidate_edges) {
            return Err(GraphError::Cycle { cycle });
        }

        self.tasks = candidate_tasks;
        self.edges = candidate_edges;
        Ok(())
    }

    /// Hard dependencies of `id`.
    pub fn hard_dependencies(&self, id: TaskId) -> Vec<TaskId> {
        self.edges
            .iter()
            .filter(|e| e.to == id && e.kind == EdgeKind::Hard)
            .map(|e| e.from)
            .collect()
    }

    /// Tasks that wait on `id`, over both edge kinds.
    pub fn dependents(&self, id: TaskId) -> Vec<TaskId> {
        self.edges
            .iter()
            .filter(|e| e.from == id)
            .map(|e| e.to)
            .collect()
    }

    /// Whether every hard dependency has completed.
    ///
    /// Only `Completed` counts. A cancelled or failed dependency does not satisfy a hard edge,
    /// because the dependent task's premise — that the earlier work exists — is false.
    pub fn dependencies_satisfied(&self, id: TaskId) -> bool {
        self.hard_dependencies(id).into_iter().all(|dep| {
            self.tasks
                .get(&dep)
                .is_some_and(|t| t.status == TaskStatus::Completed)
        })
    }

    /// Tasks eligible to be assigned, in no particular order. Priority and capacity are the
    /// scheduler's concern.
    pub fn ready(&self) -> Vec<TaskId> {
        self.tasks
            .values()
            .filter(|t| {
                matches!(t.status, TaskStatus::Backlog | TaskStatus::Queued)
                    && self.dependencies_satisfied(t.id)
            })
            .map(|t| t.id)
            .collect()
    }

    /// Everything reachable downstream of `id` over hard edges.
    ///
    /// Used to mark a failure's blast radius. Breadth-first with a visited set, so a diamond
    /// dependency is not visited twice and a cycle — should one ever slip in — cannot hang.
    pub fn hard_descendants(&self, id: TaskId) -> Vec<TaskId> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(id);

        let mut out = Vec::new();
        while let Some(current) = queue.pop_front() {
            for edge in self.edges.iter().filter(|e| e.from == current) {
                if edge.kind != EdgeKind::Hard {
                    continue;
                }
                if seen.insert(edge.to) {
                    out.push(edge.to);
                    queue.push_back(edge.to);
                }
            }
        }
        out
    }

    /// Whether the run can still make progress on its own.
    ///
    /// False means deadlock: nothing to dispatch, nothing running, nothing under review, and
    /// nobody waiting on a human. The loop must escalate rather than tick forever — a run that
    /// looks busy while achieving nothing is the most expensive failure mode available.
    pub fn progress_possible(&self, open_escalations: usize) -> bool {
        if open_escalations > 0 {
            return true;
        }
        if !self.ready().is_empty() {
            return true;
        }
        self.tasks.values().any(|t| {
            matches!(
                t.status,
                TaskStatus::Assigned | TaskStatus::Running | TaskStatus::Review
            )
        })
    }

    /// Whether every objective-gating task has completed.
    ///
    /// Deliberately a predicate over recorded state rather than a judgement: a model cannot
    /// declare an objective met.
    pub fn objective_satisfied(&self) -> bool {
        let gates: Vec<&TaskState> = self.tasks.values().filter(|t| t.objective_gate).collect();
        !gates.is_empty() && gates.iter().all(|t| t.status == TaskStatus::Completed)
    }

    /// Tasks that are blocked and can no longer be unblocked, because something they depend on
    /// is permanently terminal.
    pub fn permanently_blocked(&self) -> Vec<TaskId> {
        self.tasks
            .values()
            .filter(|t| !t.status.is_terminal())
            .filter(|t| {
                self.hard_dependencies(t.id).into_iter().any(|dep| {
                    self.tasks.get(&dep).is_some_and(|d| {
                        matches!(d.status, TaskStatus::Failed | TaskStatus::Cancelled)
                    })
                })
            })
            .map(|t| t.id)
            .collect()
    }
}

/// Returns a cycle if one exists, for the error message.
///
/// Iterative DFS with an explicit colour map: recursion would risk a stack overflow on a deep
/// graph, and a deep graph is exactly what a runaway planner produces.
fn find_cycle(tasks: &HashMap<TaskId, TaskState>, edges: &[Edge]) -> Option<Vec<TaskId>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        White,
        Grey,
        Black,
    }

    let mut adjacency: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for edge in edges {
        adjacency.entry(edge.from).or_default().push(edge.to);
    }

    let mut colour: HashMap<TaskId, Colour> = tasks.keys().map(|id| (*id, Colour::White)).collect();

    for &start in tasks.keys() {
        if colour.get(&start) != Some(&Colour::White) {
            continue;
        }

        let mut path: Vec<TaskId> = Vec::new();
        let mut stack: Vec<(TaskId, usize)> = vec![(start, 0)];
        colour.insert(start, Colour::Grey);
        path.push(start);

        while let Some((node, index)) = stack.pop() {
            let neighbours = adjacency.get(&node).cloned().unwrap_or_default();

            if index < neighbours.len() {
                stack.push((node, index + 1));
                let next = neighbours[index];

                match colour.get(&next) {
                    // Grey means it is on the current path: a cycle.
                    Some(Colour::Grey) => {
                        let start_at = path.iter().position(|n| *n == next).unwrap_or(0);
                        let mut cycle = path[start_at..].to_vec();
                        cycle.push(next);
                        return Some(cycle);
                    }
                    Some(Colour::White) => {
                        colour.insert(next, Colour::Grey);
                        path.push(next);
                        stack.push((next, 0));
                    }
                    _ => {}
                }
            } else {
                colour.insert(node, Colour::Black);
                path.pop();
            }
        }
    }

    None
}
