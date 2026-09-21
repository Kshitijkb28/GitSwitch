import { ReactNode } from "react";
import { Plus, Minus, Undo2, FileText, Box } from "lucide-react";
import { Button } from "../Button";
import type { ChangeEntry } from "../../lib/api";

/** git's one-letter codes, in words. */
export function codeLabel(code: string): string {
  switch (code) {
    case "M":
      return "modified";
    case "A":
      return "added";
    case "D":
      return "deleted";
    case "R":
      return "renamed";
    case "C":
      return "copied";
    case "T":
      return "type changed";
    case "?":
      return "untracked";
    default:
      return "changed";
  }
}

function codeColour(code: string): string {
  switch (code) {
    case "A":
      return "text-emerald-400";
    case "D":
      return "text-red-400";
    case "R":
    case "C":
      return "text-sky-400";
    case "?":
      return "text-zinc-400";
    default:
      return "text-amber-400";
  }
}

/** Why a submodule shows as changed — v2's S<c><m><u> field, in words. */
export function submoduleReason(e: ChangeEntry): string {
  const bits: string[] = [];
  if (e.sub_commit_changed) bits.push("points at a different commit");
  if (e.sub_tracked_changes) bits.push("has its own uncommitted changes");
  if (e.sub_untracked) bits.push("has untracked files");
  return bits.join(", ");
}

interface Props {
  title: string;
  icon: ReactNode;
  entries: ChangeEntry[];
  /** Which side's code to show: the staged or the worktree one. */
  side: "staged" | "unstaged";
  emptyText?: string;
  headerAction?: ReactNode;
  busy?: boolean;
  onOpen: (e: ChangeEntry) => void;
  onStage?: (paths: string[]) => void;
  onUnstage?: (paths: string[]) => void;
  onDiscard?: (paths: string[]) => void;
  accent?: "default" | "danger";
}

export function ChangesList({
  title,
  icon,
  entries,
  side,
  emptyText,
  headerAction,
  busy = false,
  onOpen,
  onStage,
  onUnstage,
  onDiscard,
  accent = "default",
}: Props) {
  if (entries.length === 0 && !emptyText) return null;

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40">
      <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-zinc-700/50">
        <div className="flex items-center gap-2 min-w-0">
          <span className={accent === "danger" ? "text-red-400" : "text-zinc-400"}>
            {icon}
          </span>
          <h3 className="text-sm font-medium text-zinc-200 truncate min-w-0">{title}</h3>
          <span className="text-xs text-zinc-500 shrink-0">{entries.length}</span>
        </div>
        {headerAction}
      </div>

      {entries.length === 0 ? (
        <p className="px-3 py-3 text-xs text-zinc-500">{emptyText}</p>
      ) : (
        <ul className="max-h-64 overflow-y-auto divide-y divide-zinc-800/70">
          {entries.map((e) => {
            const code = side === "staged" ? e.staged : e.unstaged;
            const added = side === "staged" ? e.staged_added : e.unstaged_added;
            const removed = side === "staged" ? e.staged_removed : e.unstaged_removed;
            return (
              <li key={`${side}:${e.path}`} className="flex items-center gap-2 px-3 py-1.5">
                <button
                  onClick={() => onOpen(e)}
                  className="flex items-center gap-2 min-w-0 flex-1 text-left cursor-pointer group"
                  title={
                    e.is_submodule
                      ? `Submodule — ${submoduleReason(e)}`
                      : `${codeLabel(code)} · click to see the changes`
                  }
                >
                  <span
                    className={`font-mono text-xs w-3 shrink-0 ${codeColour(code)}`}
                  >
                    {code === "." ? "·" : code}
                  </span>
                  {e.is_submodule ? (
                    <Box size={13} className="shrink-0 text-zinc-500" />
                  ) : (
                    <FileText size={13} className="shrink-0 text-zinc-600" />
                  )}
                  <span className="text-xs font-mono text-zinc-300 truncate min-w-0 group-hover:text-zinc-100">
                    {e.path}
                  </span>
                  {e.conflict && (
                    <span className="text-[10px] text-red-400 shrink-0">{e.conflict}</span>
                  )}
                  {e.is_binary && (
                    <span className="text-[10px] text-zinc-500 shrink-0">binary</span>
                  )}
                  {e.is_submodule && (
                    // Said out loud, not just in a tooltip: a submodule row has
                    // no line counts, so without this it looks like nothing
                    // happened to it.
                    <span className="text-[10px] text-sky-300/80 truncate min-w-0">
                      {submoduleReason(e)}
                    </span>
                  )}
                  {!e.is_binary && (added != null || removed != null) && (
                    <span className="text-[10px] font-mono shrink-0">
                      <span className="text-emerald-400">+{added ?? 0}</span>{" "}
                      <span className="text-red-400">−{removed ?? 0}</span>
                    </span>
                  )}
                </button>

                <div className="flex items-center gap-1 shrink-0">
                  {onStage && (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => onStage([e.path])}
                      title={e.kind === "conflicted" ? "Mark resolved" : "Stage"}
                      className="px-1.5"
                    >
                      <Plus size={13} />
                    </Button>
                  )}
                  {onUnstage && (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => onUnstage([e.path])}
                      title="Unstage"
                      className="px-1.5"
                    >
                      <Minus size={13} />
                    </Button>
                  )}
                  {onDiscard && (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => onDiscard([e.path])}
                      title="Discard — this cannot be undone"
                      className="px-1.5 hover:text-red-400"
                    >
                      <Undo2 size={13} />
                    </Button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
