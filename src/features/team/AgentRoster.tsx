import { Terminal } from "lucide-react";
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
}: {
  agents: AgentSummary[];
  onOpenSession: (sessionId: string) => void;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      {agents.map((agent, i) => (
        <AgentRow key={agent.id} agent={agent} index={i} onOpenSession={onOpenSession} />
      ))}
    </div>
  );
}

function AgentRow({
  agent,
  index,
  onOpenSession,
}: {
  agent: AgentSummary;
  index: number;
  onOpenSession: (sessionId: string) => void;
}) {
  const running = agent.status === "running";
  const blocked = agent.status === "blocked";

  return (
    <div
      className={cn(
        "animate-rise flex items-center gap-4 rounded-[9px] border px-4 py-[11px] transition-colors",
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
        {agent.session_id && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onOpenSession(agent.session_id!)}
                className="-mr-1"
              >
                <Terminal />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Read what this agent is doing.</TooltipContent>
          </Tooltip>
        )}
      </div>
    </div>
  );
}

function statusSentence(status: string): string {
  if (status === "blocked") return "Blocked — waiting on a decision";
  if (status === "idle") return "Idle — no task assigned";
  return "Waiting for work";
}
