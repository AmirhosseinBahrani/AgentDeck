# AgentDeck

A desktop app that runs many Claude Code agents as a persistent software-engineering team.

A **Supervisor** takes an objective, decomposes it into a task graph, assigns tasks to role-based
worker agents, monitors them, reviews their deliverables against explicit contracts, and loops until
the objective is met or a human is needed. Agents are long-running workers with identity, task queues
and isolated git worktrees — not chat tabs.

## Architecture

```
src/                      React + TS dashboard (Team View is the default surface, not a chat)
src-tauri/                Tauri shell + IPC command surface
crates/deck-core/         runtime, event bus, scheduler, git worktrees, permissions, persistence
crates/deck-supervisor/   deterministic control loop
docs/spec.md              product specification
```

Two principles the code is organized around:

1. **Not a chat-tab app.** The durable model is Objective → Supervisor Run → Task Graph → Agents →
   Sessions. Tabs are one way to observe sessions.
2. **The Supervisor is a deterministic controller around Claude, not one large prompt.** The
   application owns state transitions, retries, scheduling, permissions and completion detection.
   Claude supplies reasoning only at bounded, schema-validated decision points.

## Runtime

Agents are `claude` CLI subprocesses driven over duplex NDJSON
(`--input-format stream-json --output-format stream-json`). No API key is required — spawned
processes inherit OAuth credentials from the OS keychain. Notable constraints, all verified against
the CLI and pinned by fixture tests in `crates/deck-core/tests/`:

- `--verbose` is mandatory alongside `stream-json` output.
- The host assigns `--session-id`; session identity is never scraped from output.
- Sessions bucket by working directory, so `--resume` must run in the original cwd. **Removing a
  worktree makes its sessions unresumable.**
- `result` is emitted per *turn* and `total_cost_usd` is per-turn, so usage must be summed.
- `--json-schema` validates only the final result, which is why worker progress reports go through
  an in-process MCP server instead.
- Runtime tool approval is `--permission-prompt-tool stdio`, which surfaces `can_use_tool` control
  requests in-band — the same mechanism the TypeScript SDK's `canUseTool` compiles to.
- Every spawn pins `--setting-sources '' --strict-mcp-config` and an explicit `--tools`, so agents
  never inherit the developer's plugins, hooks or MCP servers. This also cut cache-creation tokens
  from ~31k to ~8k per turn.

## What the supervisor guarantees

These are enforced in code, not by prompting, and each has tests that fail if the guarantee is
removed:

- **Completion is a predicate, not a judgement.** `claim_task_done` only *queues* verification.
  The supervisor runs the task's acceptance criteria itself and a failing command means a failed
  review — the reviewer is never invoked, so no model can argue past a red test.
- **A run is not complete until the branches merge.** Every task passes in its own worktree, which
  says nothing about whether the branches work together. The final gate merges them into a
  disposable tree and runs the project's tests there. A conflict is reported, never resolved
  automatically: resolving it means choosing whose work to discard.
- **An agent that exits without claiming done has failed.** It has not quietly succeeded. The
  attempt is spent, and a task that keeps losing its agent stops and asks for a human.
- **Autonomy is enforced at dispatch**, the single point where a process gets a worktree with edit
  rights — not in the UI. An approval authorises one start, not the task, so a retry needs a new
  one.
- **Force-kill never waits on the agent.** The one most in need of killing is the one that has
  stopped answering. The worktree and its changes survive, and the task is cancelled rather than
  failed, so it does not consume a retry.

## Running unattended

Closing the window keeps the run alive; quitting from the tray is a separate, deliberate action
that stops the agents first. On Unix a process group outlives its parent, so a crash leaves agents
running — every spawn is recorded before it is given work, and startup kills what a previous
launch left behind, while leaving a concurrent instance's agents alone.

Sessions are persisted with the directory they ran in, because Claude buckets conversations by
working directory and `--resume` only works from there.

## Development

```sh
pnpm install
cargo test              # core is testable without linking Tauri
pnpm tauri dev
```

One PR per coherent bundle, squash-merged to keep `main` linear.
