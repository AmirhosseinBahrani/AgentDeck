# Summary

<!-- What changed and, more importantly, why. Link the milestone (M0-M8) this belongs to. -->

## Milestone

<!-- e.g. M1 — Spawn and stream one real agent. Note the parent branch if this PR is stacked. -->

- Milestone:
- Stacked on: `main`

## Test plan

<!--
Copy the milestone's verification checklist from the plan and tick what you actually ran.
Milestones are defined to be independently demonstrable, so this should be concrete
observed behaviour, not "should work".
-->

- [ ] `cargo test -p deck-core -p deck-supervisor`
- [ ] `cargo clippy --all-targets` clean
- [ ] `pnpm typecheck`
- [ ] Manual end-to-end check (describe what you did and what you saw):

## Runtime contract

<!-- Only if this PR touches crates/deck-core/src/runtime/. -->

- [ ] N/A — does not touch the runtime layer
- [ ] Fixtures under `crates/deck-core/tests/fixtures/` still pass unchanged
- [ ] New/changed CLI flags are covered by an `argv.rs` test
- [ ] Unknown event types still degrade to passthrough rather than failing a session
- [ ] Verified against `claude --version`:

## Safety

<!-- Only if this PR touches permissions, git worktrees, or process control. -->

- [ ] N/A
- [ ] Agents still cannot write outside their worktree
- [ ] No worktree is removed while dirty, and removal's effect on session resumability is handled
- [ ] Force-kill still terminates the whole process tree and marks the task `cancelled`, not `failed`
- [ ] No secrets are inherited into spawned processes (`--setting-sources ''` and env allowlist intact)

## Notes for the reviewer

<!-- Anything deliberately deferred, any known rough edge, any decision you want challenged. -->
