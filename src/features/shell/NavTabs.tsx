import { cn } from "../../lib/utils";

/**
 * The primary navigation, from the design.
 *
 * Shell-level and always visible, because the previous arrangement had no way back: opening a
 * session replaced the whole view and the only route home was a link tucked into the tab strip.
 * A view you can enter and not leave is a trap, and navigation belongs above the views rather
 * than inside one of them.
 *
 * `ready` stays in the shape even with every tab built: a tab that is present but does nothing
 * reads as a bug, so anything added ahead of its backend says so rather than pretending.
 */
export type NavTab =
  | "team"
  | "workspace"
  | "graph"
  | "diffs"
  | "decisions"
  | "supervisor"
  | "memory"
  | "skills";

const TABS: { id: NavTab; label: string; ready: boolean; why?: string }[] = [
  { id: "team", label: "Team", ready: true },
  { id: "workspace", label: "Sessions", ready: true },
  { id: "graph", label: "Task graph", ready: true },
  { id: "diffs", label: "Diffs", ready: true },
  { id: "decisions", label: "Decisions", ready: true },
  { id: "supervisor", label: "Supervisor", ready: true },
  { id: "memory", label: "Memory", ready: true },
  { id: "skills", label: "Skills", ready: true },
];

export function NavTabs({
  active,
  onChange,
  trailing,
}: {
  active: NavTab;
  onChange: (tab: NavTab) => void;
  trailing?: React.ReactNode;
}) {
  return (
    <nav className="flex h-[42px] shrink-0 items-center gap-5 border-b border-white/[0.07] px-7">
      {TABS.map((tab) => (
        <button
          key={tab.id}
          disabled={!tab.ready}
          title={tab.why}
          onClick={() => onChange(tab.id)}
          className={cn(
            "flex h-[42px] items-center border-b-[1.5px] text-[12.5px] transition-colors",
            active === tab.id
              ? "border-deck-live font-semibold text-deck-text"
              : "border-transparent",
            tab.ready && active !== tab.id && "text-deck-faint hover:text-deck-dim",
            !tab.ready && "cursor-not-allowed text-deck-faint/40",
          )}
        >
          {tab.label}
        </button>
      ))}
      <div className="grow" />
      {trailing}
    </nav>
  );
}
