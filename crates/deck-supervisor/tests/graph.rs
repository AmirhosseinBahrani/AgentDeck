//! Dependency graph behaviour.
//!
//! Weighted toward the two things that would quietly ruin a run: a cycle getting in, and
//! deadlock going undetected while the loop keeps ticking.

use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::domain::task::{apply, TaskEvent, TaskState, TaskStatus};
use deck_supervisor::graph::{Edge, EdgeKind, GraphError, Mutation, TaskGraph};

fn task() -> TaskState {
    TaskState::new(TaskId::new())
}

fn gate() -> TaskState {
    let mut t = task();
    t.objective_gate = true;
    t
}

fn hard(from: TaskId, to: TaskId) -> Edge {
    Edge {
        from,
        to,
        kind: EdgeKind::Hard,
    }
}

fn soft(from: TaskId, to: TaskId) -> Edge {
    Edge {
        from,
        to,
        kind: EdgeKind::Soft,
    }
}

/// Drives a task to Completed through the real state machine, so the graph is never fed a
/// status the lifecycle would not produce.
fn complete(graph: &mut TaskGraph, id: TaskId) {
    let mut s = graph.get(id).cloned().expect("task present");
    if s.status == TaskStatus::Backlog {
        s = apply(&s, TaskEvent::Enqueued).unwrap();
    }
    s = apply(
        &s,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap();
    s = apply(&s, TaskEvent::Started).unwrap();
    s = apply(&s, TaskEvent::ClaimedDone).unwrap();
    s = apply(&s, TaskEvent::ReviewPassed).unwrap();
    graph.set_state(s);
}

fn fail_permanently(graph: &mut TaskGraph, id: TaskId) {
    let mut s = graph.get(id).cloned().expect("task present");
    s.status = TaskStatus::Failed;
    graph.set_state(s);
}

#[test]
fn a_task_is_ready_only_once_its_hard_dependencies_complete() {
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);

    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    assert_eq!(g.ready(), vec![a_id], "only the root should be ready");

    complete(&mut g, a_id);
    assert_eq!(g.ready(), vec![b_id], "b should unlock once a completes");
}

#[test]
fn a_failed_dependency_does_not_satisfy_a_hard_edge() {
    // The dependent task's premise is that the earlier work exists. A failed dependency makes
    // that false, so treating it as "done" would dispatch work built on nothing.
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);
    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    fail_permanently(&mut g, a_id);

    assert!(!g.dependencies_satisfied(b_id));
    assert!(!g.ready().contains(&b_id));
}

#[test]
fn soft_edges_do_not_gate_readiness() {
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);
    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![soft(a_id, b_id)],
    })
    .unwrap();

    let ready = g.ready();
    assert!(
        ready.contains(&a_id) && ready.contains(&b_id),
        "a soft edge expresses preference, not a gate: {ready:?}"
    );
}

#[test]
fn a_direct_cycle_is_rejected() {
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);
    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    let err = g.apply(Mutation {
        add_tasks: vec![],
        add_edges: vec![hard(b_id, a_id)],
    });
    assert!(matches!(err, Err(GraphError::Cycle { .. })), "got {err:?}");
}

#[test]
fn a_long_cycle_is_rejected() {
    // Cycles a planner actually produces are rarely two nodes; they wander through several
    // tasks that each look reasonable on their own.
    let mut g = TaskGraph::new();
    let ids: Vec<TaskId> = (0..6).map(|_| TaskId::new()).collect();
    let tasks: Vec<TaskState> = ids.iter().map(|id| TaskState::new(*id)).collect();
    let chain: Vec<Edge> = ids.windows(2).map(|w| hard(w[0], w[1])).collect();

    g.apply(Mutation {
        add_tasks: tasks,
        add_edges: chain,
    })
    .unwrap();

    let err = g.apply(Mutation {
        add_tasks: vec![],
        add_edges: vec![hard(*ids.last().unwrap(), ids[0])],
    });
    assert!(matches!(err, Err(GraphError::Cycle { .. })), "got {err:?}");
}

#[test]
fn a_cycle_introduced_by_the_batch_itself_is_caught() {
    // Neither the before nor the after state shows this cycle in isolation, which is why
    // validation runs against the post-mutation adjacency rather than edge by edge.
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let c = task();
    let (a_id, b_id, c_id) = (a.id, b.id, c.id);

    let err = g.apply(Mutation {
        add_tasks: vec![a, b, c],
        add_edges: vec![hard(a_id, b_id), hard(b_id, c_id), hard(c_id, a_id)],
    });
    assert!(matches!(err, Err(GraphError::Cycle { .. })), "got {err:?}");
}

#[test]
fn a_rejected_mutation_leaves_the_graph_completely_unchanged() {
    // All-or-nothing matters because a half-applied plan is harder to reason about than a
    // rejected one, and the planner can simply be asked again.
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);
    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    let tasks_before = g.len();
    let edges_before = g.edges().len();

    // A batch whose first task is fine but whose edge closes a cycle.
    let extra = task();
    let extra_id = extra.id;
    let err = g.apply(Mutation {
        add_tasks: vec![extra],
        add_edges: vec![hard(b_id, extra_id), hard(extra_id, a_id)],
    });

    assert!(err.is_err());
    assert_eq!(g.len(), tasks_before, "no task should have been added");
    assert_eq!(
        g.edges().len(),
        edges_before,
        "no edge should have been added"
    );
    assert!(g.get(extra_id).is_none());
}

#[test]
fn self_dependency_is_rejected_distinctly_from_a_cycle() {
    let mut g = TaskGraph::new();
    let a = task();
    let a_id = a.id;
    g.apply(Mutation {
        add_tasks: vec![a],
        add_edges: vec![],
    })
    .unwrap();

    let err = g.apply(Mutation {
        add_tasks: vec![],
        add_edges: vec![hard(a_id, a_id)],
    });
    assert!(
        matches!(err, Err(GraphError::SelfDependency(_))),
        "a clearer error than a one-node cycle: {err:?}"
    );
}

#[test]
fn an_edge_to_an_unknown_task_is_rejected() {
    // Otherwise a planner typo would create a dependency on nothing, which never resolves and
    // silently strands the dependent task.
    let mut g = TaskGraph::new();
    let a = task();
    let a_id = a.id;
    g.apply(Mutation {
        add_tasks: vec![a],
        add_edges: vec![],
    })
    .unwrap();

    let err = g.apply(Mutation {
        add_tasks: vec![],
        add_edges: vec![hard(a_id, TaskId::new())],
    });
    assert!(
        matches!(err, Err(GraphError::UnknownTask(_))),
        "got {err:?}"
    );
}

#[test]
fn a_diamond_dependency_resolves_without_visiting_a_node_twice() {
    let mut g = TaskGraph::new();
    let root = task();
    let left = task();
    let right = task();
    let join = task();
    let (r, l, rt, j) = (root.id, left.id, right.id, join.id);

    g.apply(Mutation {
        add_tasks: vec![root, left, right, join],
        add_edges: vec![hard(r, l), hard(r, rt), hard(l, j), hard(rt, j)],
    })
    .unwrap();

    let mut descendants = g.hard_descendants(r);
    descendants.sort();
    let mut expected = vec![l, rt, j];
    expected.sort();
    assert_eq!(descendants, expected, "join should appear exactly once");

    complete(&mut g, r);
    complete(&mut g, l);
    assert!(
        !g.ready().contains(&j),
        "the join must wait for both branches"
    );
    complete(&mut g, rt);
    assert!(g.ready().contains(&j));
}

#[test]
fn a_failures_blast_radius_covers_transitive_dependents() {
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let c = task();
    let (a_id, b_id, c_id) = (a.id, b.id, c.id);
    g.apply(Mutation {
        add_tasks: vec![a, b, c],
        add_edges: vec![hard(a_id, b_id), hard(b_id, c_id)],
    })
    .unwrap();

    let affected = g.hard_descendants(a_id);
    assert!(affected.contains(&b_id) && affected.contains(&c_id));

    fail_permanently(&mut g, a_id);
    let blocked = g.permanently_blocked();
    assert!(
        blocked.contains(&b_id),
        "the immediate dependent should be reported as blocked"
    );
}

#[test]
fn deadlock_is_detected_rather_than_spun_on() {
    // The most expensive failure mode: a loop that looks busy while achieving nothing.
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);
    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    assert!(
        g.progress_possible(0),
        "a is ready, so progress is possible"
    );

    // a fails permanently: b can never become ready, and nothing is running.
    fail_permanently(&mut g, a_id);
    assert!(
        !g.progress_possible(0),
        "with a failed root and nothing running, the run is stuck and must escalate"
    );
    assert!(!g.ready().contains(&b_id));
}

#[test]
fn an_open_escalation_counts_as_progress_being_possible() {
    // Waiting on a human is not deadlock. Treating it as such would abandon runs that only
    // needed an answer.
    let mut g = TaskGraph::new();
    let a = task();
    let a_id = a.id;
    g.apply(Mutation {
        add_tasks: vec![a],
        add_edges: vec![],
    })
    .unwrap();
    fail_permanently(&mut g, a_id);

    assert!(!g.progress_possible(0));
    assert!(
        g.progress_possible(1),
        "a pending human decision means the run is waiting, not stuck"
    );
}

#[test]
fn running_work_counts_as_progress_even_with_nothing_ready() {
    let mut g = TaskGraph::new();
    let a = task();
    let a_id = a.id;
    g.apply(Mutation {
        add_tasks: vec![a],
        add_edges: vec![],
    })
    .unwrap();

    let s = g.get(a_id).cloned().unwrap();
    let s = apply(&s, TaskEvent::Enqueued).unwrap();
    let s = apply(
        &s,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap();
    let s = apply(&s, TaskEvent::Started).unwrap();
    g.set_state(s);

    assert!(g.ready().is_empty());
    assert!(g.progress_possible(0), "an agent is working");
}

#[test]
fn the_objective_is_met_only_when_every_gate_task_completes() {
    // A code predicate, not a judgement: a model must not be able to declare success.
    let mut g = TaskGraph::new();
    let gate_a = gate();
    let gate_b = gate();
    let side = task();
    let (ga, gb, s) = (gate_a.id, gate_b.id, side.id);

    g.apply(Mutation {
        add_tasks: vec![gate_a, gate_b, side],
        add_edges: vec![],
    })
    .unwrap();

    assert!(!g.objective_satisfied());

    complete(&mut g, ga);
    assert!(!g.objective_satisfied(), "one gate is not enough");

    complete(&mut g, gb);
    assert!(
        g.objective_satisfied(),
        "non-gating work should not hold the objective open"
    );
    assert_eq!(
        g.get(s).unwrap().status,
        TaskStatus::Backlog,
        "the side task is genuinely still incomplete"
    );
}

#[test]
fn a_graph_with_no_gate_tasks_is_never_satisfied() {
    // Otherwise an empty or gate-less plan would report success immediately.
    let mut g = TaskGraph::new();
    let a = task();
    let a_id = a.id;
    g.apply(Mutation {
        add_tasks: vec![a],
        add_edges: vec![],
    })
    .unwrap();
    complete(&mut g, a_id);

    assert!(
        !g.objective_satisfied(),
        "with nothing marked as gating, completion is undefined and must not be claimed"
    );
}

#[test]
fn adding_the_same_edge_twice_is_idempotent() {
    let mut g = TaskGraph::new();
    let a = task();
    let b = task();
    let (a_id, b_id) = (a.id, b.id);
    g.apply(Mutation {
        add_tasks: vec![a, b],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    g.apply(Mutation {
        add_tasks: vec![],
        add_edges: vec![hard(a_id, b_id)],
    })
    .unwrap();

    assert_eq!(
        g.edges().len(),
        1,
        "a replan re-stating a dependency should not duplicate it"
    );
}
