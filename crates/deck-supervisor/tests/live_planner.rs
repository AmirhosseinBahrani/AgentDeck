//! Drives one real planning decision against the installed `claude` CLI.
//!
//! Ignored by default because it spends the account's rate limit; run with
//! `cargo test -p deck-supervisor --test live_planner -- --ignored`.
//!
//! It exists because the bug it catches was invisible to everything else. Passing `--verbose`
//! alongside `--output-format json` changes the response from the result object to an array of
//! every event, so reading `structured_output` off it yielded nothing, every plan failed, no
//! task was ever created — and the app simply sat there having apparently done nothing. Unit
//! tests over captured fixtures cannot notice the day the real CLI stops matching the fixture.

use deck_supervisor::cli_planner::CliPlanner;
use deck_supervisor::decision::plan_schema;
use deck_supervisor::planner::{ModelCall, Planner};

#[tokio::test]
#[ignore = "spends rate limit; run explicitly"]
async fn the_real_cli_returns_a_plan_we_can_read() {
    let planner = CliPlanner::new(std::env::temp_dir());

    let response = planner
        .call(ModelCall {
            prompt: "Decompose this objective into exactly one task: add a README file to the \
                     repository. The task's role must be \"developer\" and it must be an \
                     objective gate."
                .into(),
            schema: plan_schema(),
            max_budget_usd: 1.0,
        })
        .await
        .expect("the planner must return a readable response");

    let tasks = response
        .structured
        .get("tasks")
        .and_then(|t| t.as_array())
        .expect("a plan has tasks");
    assert!(!tasks.is_empty(), "the model should have proposed work");

    // Cost is summed per turn across a run, so a missing value would silently under-report spend.
    assert!(
        response.cost_usd.is_some(),
        "the response must carry its cost"
    );
}
