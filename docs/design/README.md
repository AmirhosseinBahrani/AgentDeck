# Paper design reference

Source: [Zippy sunset](https://app.paper.design/file/01KYY7R7BS7FF3A64NDE5G1GQQ/1-0)

Exported from Paper through its MCP server so the implementation can be checked against exact
values rather than eyeballed from a screenshot. Committed because the export is rate-limited —
the account hit its weekly screenshot cap partway through, and a reviewer should be able to see
what the UI was built against without spending someone's quota.

The design is built on this repo's own tokens (`--color-deck-*`, Instrument Sans, JetBrains Mono),
so the JSX ports over almost directly.

## Screens

| File | Backend support |
|---|---|
| `team-view.jsx` | **built** — roster, task graph, escalations, scheduler |
| `new-objective.jsx` | **built** — the start screen |
| `runtime-check.jsx` | **built** — the readiness gate |
| `workspace.jsx` | partial — transcript and permissions exist; diff counts and the composer do not |
| `workspace-sessions.jsx` | partial — sessions exist and are reachable |
| `workspace-task-graph.jsx` | partial — edges are exposed; this is a fuller view of them |
| `workspace-decisions.jsx` | partial — the decision log exists and is persisted |
| `workspace-diffs.jsx` | **no backend** — nothing computes per-task diffs yet |
| `hire-agent.jsx` | **no backend** — the team is three fixed seeded roles |
| `revoke-agent.jsx` | **no backend** — same |
| `supervisor-conversation.jsx` | **no backend, and a design decision** — see below |

## The supervisor has no conversational surface

`supervisor-conversation.jsx` describes chatting with the supervisor. The supervisor deliberately
has no long-lived model session: every decision is a one-shot `claude -p --json-schema` call with
a code-assembled prompt, and its memory is the database. That is what makes decisions replayable
and stops an unconstrained prompt accumulating state.

A chat box wired into it would reintroduce exactly what that design removes, and would also be a
free-text channel into the permission model — the thing escalations were made typed to avoid.
Building the screen is straightforward; deciding what it is allowed to *do* is a product change,
not a styling one.
