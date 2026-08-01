import { UserMinus } from "lucide-react";
import { Button } from "../../components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../components/ui/tooltip";
import type { AgentSummary } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * Who is on the team and what each of them is doing right now.
 *
 * One row per member rather than per task: an agent between tasks still exists and still holds a
 * concurrency slot, and a roster that hid them would make the team look smaller than it is
 * whenever it was idle — exactly when you are asking why nothing is moving.
 *
 * Fixed-width slots for the dot, the name and the trailing state. Rows have to align into
 * columns down the list, and gap alone does not do that once names differ in length.
 */
export function AgentRoster({
  agents,
  onOpenSession,
  onRevoke,
}: {
  agents: AgentSummary[];
  onOpenSession: (sessionId: string) => void;
  onRevoke: (agentId: string) => void;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      {agents.map((agent, i) => (
        <AgentRow
          key={agent.id}
          agent={agent}
          index={i}
          onOpenSession={onOpenSession}
          onRevoke={onRevoke}
        />
      ))}
    </div>
  );
}

function AgentRow({
  agent,
  index,
  onOpenSession,
  onRevoke,
}: {
  agent: AgentSummary;
  index: number;
  onOpenSession: (sessionId: string) => void;
  onRevoke: (agentId: string) => void;
}) {
  // Reviewing counts as active for colour and motion — the task is in flight — but the roster
  // says which, because "finished, waiting on a verdict" and "still typing" are different things
  // to an operator deciding whether to intervene.
  const running = agent.status === "running" || agent.status === "reviewing";
  const blocked = agent.status === "blocked";
  const open = agent.session_id ? () => onOpenSession(agent.session_id!) : undefined;

  return (
    <div
      // The whole row opens the agent's transcript. Reading what an agent is doing is the reason
      // to look at a roster at all, so it should not be a small target at the end of the row.
      role={open ? "button" : undefined}
      tabIndex={open ? 0 : undefined}
      onClick={open}
      onKeyDown={(e) => {
        if (open && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          open();
        }
      }}
      className={cn(
        "animate-rise flex items-center gap-4 rounded-[9px] border px-4 py-[11px] transition-colors",
        open
          ? "cursor-pointer hover:brightness-125 focus-visible:ring-1 focus-visible:ring-deck-live/50 focus-visible:outline-none"
          : "cursor-default",
        blocked
          ? "border-deck-attention/30 bg-deck-attention/[0.06]"
          : running
            ? "border-white/[0.07] bg-white/[0.035] hover:bg-white/[0.055]"
            : // Idle and offline recede: they are on the team but not part of what is happening,
              // and giving them equal weight would make a busy screen harder to scan.
              "border-white/[0.05] bg-white/[0.02] opacity-70",
      )}
      style={{ animationDelay: `${Math.min(index, 8) * 30}ms` }}
    >
      <span className="flex w-2 shrink-0 justify-center">
        <span
          className={cn(
            "size-[7px] rounded-full",
            running && "ring-live animate-live bg-deck-live",
            blocked && "ring-attention bg-deck-attention",
            !running && !blocked && "border border-deck-faint/70",
          )}
        />
      </span>

      <div className="flex w-[150px] shrink-0 flex-col gap-px">
        <span className="text-[13.5px] leading-[18px] font-semibold text-deck-text">
          {agent.name}
        </span>
        <span className="font-mono text-[10.5px] leading-[14px] text-deck-faint">
          {agent.role}
        </span>
      </div>

      <div className="flex min-w-0 grow flex-col gap-[3px]">
        <span className="truncate text-[12.5px] leading-4 text-deck-text">
          {agent.activity ?? statusSentence(agent.status)}
        </span>
        <span className="truncate font-mono text-[10.5px] leading-[14px] text-deck-faint">
          {[agent.task_id ? `task ${agent.task_id.slice(0, 8)}` : null, agent.branch]
            .filter(Boolean)
            .join(" · ") || "no worktree yet"}
        </span>
      </div>

      <div className="flex w-[132px] shrink-0 items-center justify-end gap-2">
        {/* Attempt and review counts, not a percentage. Nothing in the system reports how far
            through a task an agent is, and a made-up bar would misreport real work — these are
            the two numbers that genuinely move as a task struggles. */}
        {agent.attempts > 0 && (
          <span className="font-mono text-[10.5px] text-deck-faint">
            try {agent.attempts}
            {agent.review_rounds > 0 && ` · rev ${agent.review_rounds}`}
          </span>
        )}
        <span
          className={cn(
            "font-mono text-[11px]",
            running && "text-deck-live",
            blocked && "text-deck-attention",
            !running && !blocked && "text-deck-faint",
          )}
        >
          {agent.status}
        </span>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              onClick={(e) => {
                e.stopPropagation();
                onRevoke(agent.id);
              }}
            >
              <UserMinus />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Take this agent off the roster.</TooltipContent>
        </Tooltip>
      </div>
    </div>
  );
}

function statusSentence(status: string): string {
  if (status === "blocked") return "Blocked — waiting on a decision";
  if (status === "reviewing") return "Done — waiting on the reviewer";
  if (status === "idle") return "Idle — no task assigned";
  return "Waiting for work";
}
