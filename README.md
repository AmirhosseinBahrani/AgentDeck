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

## Development

```sh
pnpm install
cargo test              # core is testable without linking Tauri
pnpm tauri dev
```

Work proceeds in milestones (M0–M8), one PR each, squash-merged to keep `main` linear.
