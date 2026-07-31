# ClaudeDock — Desktop Agent Workspace Specification

## 1. Product Definition

ClaudeDock is a desktop application for running and managing multiple Claude development agents as a persistent software-engineering team.

The key model is **not** "multiple Claude chat tabs." Each agent should behave like a real developer/employee with a role, context, workspace, tools, tasks, memory, execution loop, and accountability. A **Supervisor Agent** sits above the developer agents, continuously managing the team in a loop: planning work, assigning tasks, monitoring execution, reviewing results, handling blockers, reassigning work, and deciding what happens next.

The product therefore evolves from a Claude tab manager into a **local-first AI engineering team operating system**.

---

## 2. Core Mental Model

```text
Workspace
  └── Project
       ├── Supervisor
       │    └── manages team in a continuous loop
       │
       ├── Agent: Architect
       ├── Agent: Backend Developer
       ├── Agent: Frontend Developer
       ├── Agent: QA Engineer
       ├── Agent: Security Reviewer
       └── Agent: DevOps Engineer

Tasks
  ↓
Supervisor
  ↓
Assign / prioritize / coordinate
  ↓
Developer Agents
  ↓
Implement / test / report
  ↓
Supervisor reviews outcomes
  ↓
New tasks / fixes / escalation / completion
  ↓
Repeat until goal is complete
```

The system should model agents as **long-running workers**, not disposable conversations.

---

## 3. Product Goals

### Primary goals

1. Run many Claude agents concurrently.
2. Make each agent behave like an independent developer/employee.
3. Provide a persistent Supervisor Agent responsible for coordinating the team.
4. Support continuous agent execution loops.
5. Make tasks, ownership, dependencies, and status explicit.
6. Isolate agents with Git worktrees where appropriate.
7. Provide permission and tool governance.
8. Persist all important state locally.
9. Allow agents to operate in the background while the user works elsewhere.
10. Make the user's role closer to a manager/reviewer than a manual operator.

### Non-goals for MVP

- Building a new LLM inference engine.
- Reimplementing Claude Code's agent runtime.
- Cloud collaboration.
- A complete replacement for VS Code.
- Complex visual workflow editing before the core supervisor loop is reliable.

---

## 4. Core Concepts

### 4.1 Workspace

Top-level container containing projects, teams, agent definitions, tasks, sessions, MCP configuration, and policies.

```text
Workspace
├── Projects
├── Team
├── Agents
├── Tasks
├── Sessions
├── MCP Servers
└── Policies
```

### 4.2 Project

A local repository or directory that agents work on.

```typescript
interface Project {
  id: string
  workspaceId: string
  name: string
  path: string
  repository?: string
  defaultBranch: string
}
```

### 4.3 Agent

An agent is a persistent worker definition. It represents a role and its operating policy rather than a single conversation.

```typescript
interface AgentDefinition {
  id: string
  workspaceId: string
  name: string
  role: AgentRole
  description?: string

  model: ModelConfig
  systemPrompt?: string

  projectId?: string
  workingDirectory?: string

  permissionPolicy: PermissionPolicy
  allowedTools: ToolPermission[]
  mcpServers: string[]

  gitStrategy: GitStrategy
  environment: Record<string, string>

  maxConcurrentSessions: number
}
```

### 4.4 Session

A live or resumable Claude execution associated with an agent.

```typescript
interface AgentSession {
  id: string
  agentId: string
  claudeSessionId?: string
  status: SessionStatus
  taskId?: string
  worktreePath?: string
  startedAt?: number
  lastActivityAt?: number
}
```

### 4.5 Task

The unit of work owned by an agent and managed by the supervisor.

```typescript
interface Task {
  id: string
  projectId: string
  title: string
  description: string

  status:
    | 'backlog'
    | 'queued'
    | 'assigned'
    | 'running'
    | 'blocked'
    | 'review'
    | 'completed'
    | 'failed'
    | 'cancelled'

  priority: number
  assigneeAgentId?: string
  parentTaskId?: string
  dependencies: string[]

  branch?: string
  worktreePath?: string

  createdAt: number
  startedAt?: number
  completedAt?: number
}
```

---

# 5. Employee-Like Agent Model

The agent should have the lifecycle of an employee:

```text
Created
  ↓
Onboarding
  ↓
Available
  ↓
Assigned Task
  ↓
Working
  ↓
Reporting Progress
  ↓
Waiting / Blocked
  ↓
Review
  ↓
Fix / Continue
  ↓
Completed
  ↓
Available Again
```

Agents should maintain state across tasks instead of being treated as one-off prompt executions.

### Agent state

```typescript
interface AgentRuntimeState {
  agentId: string
  state:
    | 'offline'
    | 'starting'
    | 'idle'
    | 'working'
    | 'waiting'
    | 'blocked'
    | 'awaiting_review'
    | 'error'

  currentTaskId?: string
  currentSessionId?: string
  currentActivity?: string
  lastReportAt?: number
}
```

### Agent responsibilities

Each employee-like agent should have:

- role
- skills
- instructions
- tool permissions
- project context
- working directory
- Git strategy
- task queue
- current task
- execution history
- progress reporting
- failure state
- escalation rules
- persistent identity/profile

---

# 6. Supervisor Agent

The Supervisor is the central orchestration layer and should be treated as a first-class agent with a higher-level responsibility.

It is not merely another chat tab.

Its job is to manage the engineering team toward a user-defined objective.

### Supervisor responsibilities

- Understand the overall goal.
- Break goals into executable tasks.
- Decide which agent should own each task.
- Prioritize tasks.
- Track dependencies.
- Observe agent progress.
- Detect stalled or failed agents.
- Review results.
- Request fixes.
- Reassign tasks.
- Spawn additional workers when needed.
- Escalate ambiguity to the human.
- Decide when the overall objective is complete.

---

# 7. Supervisor Control Loop

The core product primitive is the **Supervisor Loop**.

```text
                    ┌──────────────────────┐
                    │    User Objective    │
                    └──────────┬───────────┘
                               ↓
                    ┌──────────────────────┐
                    │      Supervisor      │
                    │   Understand/Plan    │
                    └──────────┬───────────┘
                               ↓
                    ┌──────────────────────┐
                    │ Create / prioritize  │
                    │       tasks          │
                    └──────────┬───────────┘
                               ↓
             ┌─────────────────┴─────────────────┐
             ↓                                   ↓
      ┌──────────────┐                    ┌──────────────┐
      │ Developer A  │                    │ Developer B  │
      │    Working   │                    │    Working   │
      └──────┬───────┘                    └──────┬───────┘
             │                                   │
             └────────────────┬──────────────────┘
                              ↓
                    ┌──────────────────────┐
                    │ Collect results /    │
                    │ progress / blockers │
                    └──────────┬───────────┘
                               ↓
                    ┌──────────────────────┐
                    │ Review / evaluate    │
                    │ outcomes             │
                    └──────────┬───────────┘
                               ↓
               ┌───────────────┼────────────────┐
               ↓               ↓                ↓
           Continue          Fix             Escalate
               │               │                │
               └───────────────┼────────────────┘
                               ↓
                     Supervisor loops again
```

The loop continues until one of these terminal states occurs:

```text
SUCCESS
BLOCKED_ON_HUMAN
FAILED
CANCELLED
```

---

# 8. Supervisor Loop State Machine

```typescript
interface SupervisorRun {
  id: string
  projectId: string
  objective: string

  status:
    | 'planning'
    | 'dispatching'
    | 'monitoring'
    | 'reviewing'
    | 'replanning'
    | 'blocked_on_human'
    | 'completed'
    | 'failed'
    | 'cancelled'

  iteration: number
  startedAt: number
  updatedAt: number
}
```

A conceptual loop:

```text
while objective_not_terminal:

    observe_project_state()
    observe_agents()
    observe_tasks()
    observe_git_and_tests()

    if new_work_is_required:
        create_tasks()

    if tasks_are_unassigned:
        assign_tasks()

    if agents_are_blocked:
        resolve_or_reassign()

    if completed_work_needs_validation:
        review_results()

    if review_failed:
        create_follow_up_tasks()

    if human_decision_required:
        escalate()
        pause_loop()

    if objective_complete:
        finish_run()
    else:
        schedule_next_iteration()
```

The exact supervisor implementation should use deterministic orchestration logic around Claude rather than relying entirely on a single unconstrained prompt.

---

# 9. Human-in-the-Loop

The human becomes the manager of the manager.

The Supervisor should escalate when:

- requirements are ambiguous
- destructive operations are requested
- credentials or sensitive access are required
- architecture decisions have high impact
- agents repeatedly fail
- conflicting agent reports cannot be resolved
- external approval is required

Example:

```text
┌───────────────────────────────────────────────┐
│ Supervisor needs your decision                │
├───────────────────────────────────────────────┤
│ Two implementation approaches are possible.   │
│                                                │
│ A — keep current schema                        │
│ B — introduce event sourcing                   │
│                                                │
│ Reviewer recommends A.                         │
│                                                │
│ [ Choose A ] [ Choose B ] [ Ask Supervisor ] │
└───────────────────────────────────────────────┘
```

---

# 10. Team Structure

The workspace should support configurable teams.

Example:

```text
Supervisor
│
├── Architect
├── Backend Engineer
├── Frontend Engineer
├── QA Engineer
├── Security Engineer
└── DevOps Engineer
```

For a smaller project:

```text
Supervisor
├── Full-stack Developer
└── Reviewer
```

The user should be able to create a team template and reuse it across projects.

---

# 11. Tabs vs Agents

This distinction is fundamental.

### Tab

A UI representation of a session.

### Agent

A persistent employee/worker definition.

### Task

A unit of work assigned to an agent.

### Supervisor

A manager responsible for the team and overall objective.

Therefore:

```text
One Agent
  ├── many Sessions over its lifetime
  ├── many Tasks over its lifetime
  └── one persistent role/profile
```

And:

```text
Supervisor
  └── many Agents
       └── many Tasks
            └── many Sessions
```

---

# 12. UI / Desktop Layout

```text
┌─────────────────────────────────────────────────────────────────┐
│ ClaudeDock                                       ⌘K  Settings   │
├───────────────┬─────────────────────────────────────────────────┤
│ WORKSPACES    │ Session Tabs                                    │
│               │ [ API ] [ DB ] [ Reviewer ] [ Supervisor ] [+] │
│ ▾ Payments    ├─────────────────────────────────────────────────┤
│   backend     │                                                 │
│   frontend    │               ACTIVE SESSION                    │
│               │                                                 │
│ TEAM          │   conversation / tool calls / changes           │
│               │                                                 │
│ ★ Supervisor  │                                                 │
│ ● Backend     │                                                 │
│ ● QA          │                                                 │
│ ● Security    │                                                 │
│ ○ DevOps      │                                                 │
│               ├─────────────────────────────────────────────────┤
│ TASKS         │ Terminal / Input / Permission Requests          │
│               │                                                 │
│ ● Running     │                                                 │
│ ◐ Review      │                                                 │
│ ✓ Completed   │                                                 │
├───────────────┴─────────────────────────────────────────────────┤
│ Team: 5 agents   Running: 3   Blocked: 1   Tasks: 12   CPU: 34%│
└─────────────────────────────────────────────────────────────────┘
```

A separate **Team View** should display the organization's live state:

```text
Supervisor
    │
    ├── ● Backend      Implementing auth middleware
    ├── ● QA           Running integration tests
    ├── ◐ Security     Reviewing PR
    ├── ○ DevOps       Idle
    └── ⚠ Frontend     Blocked on API contract
```

---

# 13. Agent Dashboard

Every agent should have a profile page.

```text
Backend Engineer
────────────────────────────────────
Status       Working
Current Task JWT middleware
Project      wallet-service
Branch       agent/auth-middleware
Session      Active

Skills
  Go
  PostgreSQL
  REST
  Security

Current activity
  Running integration tests

Recent tasks
  ✓ Implement wallet lookup
  ✓ Add DB index
  ● JWT middleware
  ◐ Fix test failures
```

---

# 14. Tasks and Dependencies

The supervisor needs a real task graph, not just a flat to-do list.

```text
Architecture
    ↓
API Contract
    ↓
Backend ──────────┐
                  ↓
Frontend         Integration Tests
                  ↓
               Reviewer
                  ↓
               Release
```

Task metadata should include:

- owner
- priority
- dependency graph
- acceptance criteria
- expected outputs
- artifacts
- status
- attempts
- failure reason
- review status

---

# 15. Agent Progress Reporting

Agents should periodically report structured state to the Supervisor.

```typescript
interface AgentProgressReport {
  agentId: string
  taskId: string

  status: 'working' | 'blocked' | 'completed' | 'failed'

  summary: string
  progress?: number

  completedWork: string[]
  remainingWork: string[]
  blockers: string[]

  changedFiles: string[]
  testsRun: string[]

  needsSupervisorAction: boolean
}
```

This is important because the Supervisor should not infer everything from raw conversation text.

---

# 16. Agent Contracts

Every task should define an explicit contract.

```yaml
Task:
  title: Implement JWT authentication

  acceptance_criteria:
    - Validate signature
    - Validate expiration
    - Add unit tests
    - Add integration tests

  constraints:
    - Do not modify database schema
    - Maintain backwards compatibility

  deliverables:
    - source changes
    - tests
    - summary
```

The Supervisor evaluates the agent against this contract.

---

# 17. Review Loop

An agent completing a task is not equivalent to task completion.

```text
Developer
   ↓
Implementation
   ↓
Tests
   ↓
Reviewer
   ↓
┌──────────────┐
│ Pass?        │
└──────┬───────┘
   Yes │ No
       │
       ↓
   Complete     Developer gets
                follow-up task
```

Reviewers may be specialized agents:

- Code Reviewer
- Security Reviewer
- Test Reviewer
- Architecture Reviewer

---

# 18. Git / Worktree Isolation

Parallel developers should not blindly share one directory.

```text
repo/
├── main
└── .claudedock/
    ├── agent-backend/
    ├── agent-frontend/
    ├── agent-security/
    └── agent-tests/
```

The Supervisor can decide whether a task requires:

- isolated worktree
- shared read-only project
- shared working tree for tightly coupled tasks

Preferred default: **isolated worktree for code-changing parallel agents**.

---

# 19. Runtime Architecture

```text
┌─────────────────────────────────────┐
│             React UI                │
│ Tabs / Team / Tasks / Diff / Chat   │
└──────────────────┬──────────────────┘
                   │ Tauri IPC
                   ↓
┌─────────────────────────────────────┐
│             Rust Core               │
│                                     │
│ Workspace Manager                   │
│ Agent Manager                       │
│ Supervisor Engine                   │
│ Task Scheduler                      │
│ Session Manager                     │
│ Process Manager                     │
│ Git / Worktree Manager              │
│ Permission Manager                  │
│ Event Bus                            │
│ Persistence                         │
└───────────────┬─────────────────────┘
                │
       ┌────────┼────────┐
       ↓        ↓        ↓
   Supervisor  Agent A  Agent B ... Agent N
       │        │        │
       └────────┴────────┘
                ↓
          Claude Runtime
```

---

# 20. Supervisor Engine Architecture

The Supervisor should be implemented as a deterministic controller around Claude.

```text
Supervisor Engine
│
├── Objective Manager
├── Task Planner
├── Assignment Engine
├── Scheduler
├── Agent Monitor
├── Result Collector
├── Review Coordinator
├── Failure Handler
├── Escalation Manager
└── Completion Detector
```

Claude provides reasoning for planning and decisions, while the application enforces state transitions, permissions, scheduling, retries, and invariants.

This separation prevents the entire system from depending on an unconstrained prompt to maintain workflow correctness.

---

# 21. Agent Runtime Interface

The product should abstract the underlying Claude runtime.

```typescript
interface AgentRuntime {
  createSession(config: SessionConfig): Promise<AgentSession>
  sendMessage(sessionId: string, message: string): Promise<void>
  interrupt(sessionId: string): Promise<void>
  resume(sessionId: string): Promise<void>
  terminate(sessionId: string): Promise<void>
  events(sessionId: string): AsyncIterable<AgentEvent>
}
```

Initial implementation:

```text
AgentRuntime
    ↓
ClaudeCodeRuntime
```

Future implementations could support additional runtimes/models without rewriting the orchestration layer.

---

# 22. Event Architecture

Everything important should become an internal event.

```typescript
type AgentEvent =
  | { type: 'message'; sessionId: string; content: string }
  | { type: 'tool_call'; sessionId: string; tool: string; input: unknown }
  | { type: 'tool_result'; sessionId: string; output: unknown }
  | { type: 'permission_request'; requestId: string; tool: string; arguments: unknown }
  | { type: 'progress'; report: AgentProgressReport }
  | { type: 'task_completed'; taskId: string }
  | { type: 'task_failed'; taskId: string; reason: string }
  | { type: 'agent_blocked'; agentId: string; reason: string }
  | { type: 'session_complete'; sessionId: string }
  | { type: 'error'; message: string }
```

Event consumers:

```text
             Event Bus
          /      |      \
         /       |       \
        ↓        ↓        ↓
       UI   Persistence  Supervisor
                       \
                        ↓
                    Notifications
```

---

# 23. Permission Model

Permissions should be layered:

```text
Global
  ↓
Workspace
  ↓
Project
  ↓
Agent
  ↓
Task
  ↓
Session
  ↓
Tool Invocation
```

Example:

```text
Backend Agent
✓ Read project
✓ Edit project
✓ Run unit tests
✓ Run formatter
✗ Access SSH private keys
✗ Delete production resources
✗ Modify unrelated repositories
```

Sensitive actions should be escalated to the user or require explicit policy approval.

---

# 24. MCP Manager

MCP servers are configured centrally and assigned per agent/team.

```text
Workspace MCP
├── GitHub
├── Postgres
├── Sentry
├── Linear
└── Kubernetes
```

An agent may receive only the MCP servers required for its role.

---

# 25. Persistent Memory

Agents need role memory, but memory must be separated into explicit layers.

```text
Agent Memory
├── Role profile
├── Project knowledge
├── Previous task summaries
├── Current task context
└── Operational history
```

Do not automatically inject the entire historical transcript into every run. Store structured summaries and selectively retrieve context.

---

# 26. Local Database

SQLite is the default local store.

Core tables:

```text
workspaces
projects
agents
agent_runtime_states
supervisor_runs
tasks
task_dependencies
sessions
messages
tool_calls
permission_requests
worktrees
mcp_servers
agent_reports
reviews
events
```

### Important relationships

```text
workspace
  ├── projects
  ├── agents
  └── supervisor_runs

project
  ├── tasks
  ├── worktrees
  └── sessions

agent
  ├── sessions
  ├── tasks
  └── reports

supervisor_run
  ├── tasks
  ├── agent assignments
  └── decisions
```

---

# 27. Recommended Technology Stack

| Layer | Technology |
|---|---|
| Desktop | Tauri 2 |
| Frontend | React + TypeScript |
| Build | Vite |
| Styling | Tailwind CSS |
| Components | shadcn/ui + Radix |
| State | Zustand |
| Server/cache state | TanStack Query |
| Editor | Monaco |
| Terminal | xterm.js |
| Backend/core | Rust |
| Async runtime | Tokio |
| Serialization | serde |
| Database | SQLite |
| DB layer | SQLx |
| Git | Git CLI and/or libgit2 |
| Claude execution | Claude Code CLI / SDK |
| Protocol/tools | MCP |
| Secrets | OS keychain / credential store |
| Packaging | Tauri bundler |
| Updates | Tauri updater |

---

# 28. Why Rust + Tauri

The application is fundamentally a desktop process and orchestration system:

- process spawning
- PTYs
- filesystem access
- Git
- worktrees
- concurrent sessions
- permissions
- local persistence
- OS integration

Rust is a strong fit for this backend while React provides a flexible interface for a complex desktop UI.

Electron remains a viable alternative, but Tauri is the preferred architecture for a local-first, multi-process application with potentially many concurrent agents.

---

# 29. Claude Runtime Integration

The initial product should use Claude Code rather than recreating the coding-agent runtime.

Conceptually:

```text
ClaudeDock
    ↓
AgentRuntime abstraction
    ↓
Claude Code process / SDK
    ↓
Claude model + tools + MCP
```

Structured events should be parsed into ClaudeDock's own event model so that the rest of the application is independent of Claude Code's raw output format.

---

# 30. Process Model

Each active agent session can correspond to a managed runtime process.

```rust
struct AgentProcess {
    session_id: String,
    pid: u32,
    working_directory: PathBuf,
    status: AgentStatus,
}
```

The Process Manager is responsible for:

- startup
- shutdown
- interruption
- restart
- health monitoring
- stdout/stderr capture
- structured event parsing
- crash recovery

---

# 31. Scheduling and Concurrency

The Supervisor should not start unlimited agents.

Introduce resource-aware scheduling:

```text
Task Queue
   ↓
Scheduler
   ├── max concurrent agents
   ├── CPU budget
   ├── memory budget
   ├── model budget
   └── project constraints
```

Example:

```text
Max workers: 6

Running:
  Backend
  Frontend
  QA

Queued:
  Security
  DevOps
  Documentation
```

The scheduler should also prevent conflicting tasks from modifying the same resources concurrently.

---

# 32. Failure Handling

Agent failure must be a normal workflow state, not an application crash.

```text
Agent fails
   ↓
Capture reason
   ↓
Retry policy
   ├── retry same agent
   ├── restart session
   ├── reassign task
   └── escalate to supervisor
```

Task retry metadata:

```typescript
interface RetryPolicy {
  maxAttempts: number
  backoffMs: number
  reassignOnFailure: boolean
}
```

---

# 33. Supervisor Decision Log

The Supervisor should expose its important decisions.

```text
09:42 Supervisor
Assigned JWT task → Backend Agent
Reason: Go + auth expertise

09:48 Supervisor
Detected integration dependency
Created task → QA Agent

09:57 Supervisor
Reviewer rejected implementation
Created fix task → Backend Agent

10:12 Supervisor
All acceptance criteria satisfied
Marked objective complete
```

This makes the system inspectable and debuggable.

---

# 34. Team View

The main high-level view should feel like an engineering manager dashboard.

```text
PROJECT: Wallet Service

OBJECTIVE
Implement secure transaction signing flow

SUPERVISOR
● Planning iteration 7

TEAM
────────────────────────────────────────
Backend         ● Running      72%
Security        ● Reviewing    91%
QA              ● Testing      48%
DevOps          ○ Idle

TASKS
────────────────────────────────────────
12 completed
3 running
1 blocked
2 queued

BLOCKERS
────────────────────────────────────────
QA waiting for backend API contract

RECENT DECISIONS
────────────────────────────────────────
Security review requested before merge
```

---

# 35. User Interaction Modes

### Manual mode

The user directly controls agents.

```text
User → Agent
```

### Assisted mode

The user creates tasks while the Supervisor distributes them.

```text
User → Supervisor → Agents
```

### Autonomous mode

The user supplies an objective and the Supervisor runs the team until completion or escalation.

```text
User
 ↓
Objective
 ↓
Supervisor
 ↓
Team
 ↓
Review / rework loop
 ↓
Completion
```

The system should make the autonomy level explicit in the UI.

---

# 36. MVP Scope

## v0.1 — Core Team Runtime

- Multiple Claude sessions/tabs
- Persistent agent definitions
- Agent roles
- Supervisor agent
- Task creation and assignment
- Supervisor control loop
- Background execution
- Agent status
- Progress reporting
- Session persistence
- Claude runtime integration
- Basic permissions
- SQLite
- Git worktree support
- Basic notifications

## v0.2 — Engineering Management

- Task dependency graph
- Review agents
- Diff viewer
- Agent dashboards
- Decision log
- Global search
- Retry/reassignment policies
- MCP manager
- Team templates
- Human escalation UI

## v0.3 — Autonomous Engineering Workflows

- Multi-agent workflows
- Supervisor-driven task decomposition
- Automatic reviewer assignment
- Agent-to-agent communication
- PR creation
- GitHub integration
- CI integration
- Automatic validation loops

## v1.0 — AI Engineering Team OS

- Multi-project teams
- Advanced supervisor policies
- Persistent agent memory
- Scheduling/resource controls
- Rich workflow graphs
- Team analytics
- Cloud sync / collaboration as an optional layer
- Multiple model/runtime adapters

---

# 37. Killer Workflow

A user should eventually be able to type:

```text
Build a production-ready webhook retry system for the payments service.
```

ClaudeDock should turn that into:

```text
Supervisor
│
├── Architect
│    └── define design + acceptance criteria
│
├── Backend Developer
│    └── implement retry engine
│
├── Database Engineer
│    └── design migration / indexes
│
├── QA Engineer
│    └── build integration tests
│
└── Security Reviewer
     └── review implementation
```

Then the Supervisor runs the loop:

```text
Plan
 ↓
Assign
 ↓
Execute
 ↓
Observe
 ↓
Test
 ↓
Review
 ↓
Fix
 ↓
Re-test
 ↓
Merge / release
 ↓
Verify objective
```

The user can observe the team, intervene when needed, or let it operate autonomously within configured policies.

---

# 38. Most Important Architectural Principle

The product should not be built around the concept of a chat tab.

It should be built around:

```text
                    OBJECTIVE
                        ↓
                   SUPERVISOR
                        ↓
                  TASK GRAPH
                        ↓
                    AGENTS
                        ↓
                 EXECUTION LOOPS
                        ↓
              RESULTS / ARTIFACTS
                        ↓
                    REVIEW
                        ↓
                 REPLANNING LOOP
                        ↺
```

Tabs are only one UI surface for observing those sessions.

The durable architecture is:

```text
Workspace
  → Objective
    → Supervisor Run
      → Task Graph
        → Agent Workers
          → Claude Sessions
            → Tools / Git / MCP
              → Artifacts
                → Review
                  → Supervisor Decision
                    → Next Iteration
```

This model supports the intended experience: **a virtual software engineering organization where Claude agents behave like real developers and a Supervisor behaves like an engineering manager continuously coordinating them toward a shared objective.**
