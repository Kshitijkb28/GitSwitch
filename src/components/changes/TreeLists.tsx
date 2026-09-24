import { FilePlus2, FileEdit, ShieldAlert } from "lucide-react";
import { Button } from "../Button";
import { ChangesList } from "./ChangesList";
import type { ChangeEntry, RepoStatus, Sides } from "../../lib/api";

export type DiscardVariant = "changes" | "untracked";

/**
 * What "mine" and "theirs" mean right now. The backend says so when it can
 * (`operation.sides`); otherwise the kind of operation decides — in a rebase
 * "mine" is the commit being replayed, which surprises people every time.
 */
export function sidesFor(status: RepoStatus): Sides {
  const op = status.operation;
  if (op?.sides) return op.sides;
  const branch = status.branch;
  switch (op?.kind) {
    case "merge":
      return { mine: branch ?? "your branch", theirs: "the incoming branch" };
    case "rebase":
      return { mine: "your commit being replayed", theirs: status.upstream ?? "the base" };
  }
  if (status.conflict_source === "stash" || status.conflict_source === "autostash") {
    return { mine: branch ?? "yours", theirs: "the stashed changes" };
  }
  return { mine: branch ?? "yours", theirs: "the other side" };
}

/** Conflicts with no operation behind them: say where they came from. */
export function conflictCaption(status: RepoStatus): string | null {
  if (status.conflict_source === "stash") {
    return "These conflicts came from applying a stash; the entry is still in the list. Keep mine / Take theirs / Discard resolves them.";
  }
  if (status.conflict_source === "autostash") {
    return "These conflicts came from re-applying your stashed changes after the rebase; the entry is still in the list. Keep mine / Take theirs / Discard resolves them.";
  }
  return null;
}

type Props = {
  status: RepoStatus;
  busy: boolean;
  onOpen: (e: ChangeEntry) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  /** Asks first — the page owns the confirmation. */
  onDiscard: (paths: string[], variant: DiscardVariant) => void;
  onResolveSide: (paths: string[], side: "mine" | "theirs") => void;
  /** Gitlink rows with their own section on the page get a jump link. */
  jumpableSubmodules?: string[];
  onJumpToSubmodule?: (path: string) => void;
};

/** Conflicts · Staged · Not staged · Untracked — one repository's working tree. */
export function TreeLists({
  status,
  busy,
  onOpen,
  onStage,
  onUnstage,
  onDiscard,
  onResolveSide,
  jumpableSubmodules,
  onJumpToSubmodule,
}: Props) {
  const entries = status.entries;
  const conflicted = entries.filter((e) => e.kind === "conflicted");
  const staged = entries.filter((e) => e.kind === "tracked" && e.staged !== ".");
  const unstaged = entries.filter((e) => e.kind === "tracked" && e.unstaged !== ".");
  const untracked = entries.filter((e) => e.kind === "untracked");
  const sides = sidesFor(status);
  const caption = conflictCaption(status);
  const fromStash = status.conflict_source === "stash" || status.conflict_source === "autostash";

  return (
    <>
      {conflicted.length > 0 && (
        <ChangesList
          title="Conflicts"
          icon={<ShieldAlert size={14} />}
          accent="danger"
          entries={conflicted}
          side="unstaged"
          busy={busy}
          sides={sides}
          caption={caption}
          onOpen={onOpen}
          onStage={onStage}
          onResolveSide={onResolveSide}
          // With no operation to abort, going back to HEAD is the third way out.
          onDiscard={fromStash ? (paths) => onDiscard(paths, "changes") : undefined}
          jumpableSubmodules={jumpableSubmodules}
          onJumpToSubmodule={onJumpToSubmodule}
        />
      )}

      <ChangesList
        title="Staged"
        icon={<FileEdit size={14} />}
        entries={staged}
        side="staged"
        busy={busy}
        emptyText="Nothing staged yet."
        headerAction={
          staged.length > 0 ? (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => onUnstage(staged.map((e) => e.path))}
            >
              Unstage all
            </Button>
          ) : undefined
        }
        onOpen={onOpen}
        onUnstage={onUnstage}
        jumpableSubmodules={jumpableSubmodules}
        onJumpToSubmodule={onJumpToSubmodule}
      />

      <ChangesList
        title="Not staged"
        icon={<FileEdit size={14} />}
        entries={unstaged}
        side="unstaged"
        busy={busy}
        emptyText="No unstaged changes."
        headerAction={
          unstaged.length > 0 ? (
            <div className="flex gap-1">
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => onDiscard(unstaged.map((e) => e.path), "changes")}
                className="hover:text-red-400"
              >
                Discard all
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => onStage(unstaged.map((e) => e.path))}
              >
                Stage all
              </Button>
            </div>
          ) : undefined
        }
        onOpen={onOpen}
        onStage={onStage}
        onDiscard={(paths) => onDiscard(paths, "changes")}
        jumpableSubmodules={jumpableSubmodules}
        onJumpToSubmodule={onJumpToSubmodule}
      />

      <ChangesList
        title="Untracked"
        icon={<FilePlus2 size={14} />}
        entries={untracked}
        side="unstaged"
        busy={busy}
        emptyText="No new files."
        headerAction={
          untracked.length > 0 ? (
            <div className="flex gap-1">
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => onDiscard(untracked.map((e) => e.path), "untracked")}
                className="hover:text-red-400"
              >
                Delete all untracked
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => onStage(untracked.map((e) => e.path))}
              >
                Stage all
              </Button>
            </div>
          ) : undefined
        }
        onOpen={onOpen}
        onStage={onStage}
        onDiscard={(paths) => onDiscard(paths, "changes")}
      />

      {status.untracked_truncated && (
        <p className="text-xs text-amber-400/90">
          More than 2000 untracked files — only the first 2000 are shown. A
          .gitignore would make this folder much easier to work with.
        </p>
      )}
    </>
  );
}
