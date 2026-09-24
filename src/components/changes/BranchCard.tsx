import { RefreshCw, ArrowUp, ArrowDown, GitBranch, Upload, Download, Ban, Undo2, Archive, RotateCcw, Eraser } from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import { Checkbox } from "../Checkbox";
import { PullControl, whyDisabled } from "./PullControl";
import type { PullMode, RepoStatus } from "../../lib/api";

/** "4 minutes ago" for the last fetch, so staleness is readable at a glance. */
export function agoLabel(secs: number | null): string {
  if (secs === null) return "never fetched";
  if (secs < 90) return "fetched just now";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `fetched ${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 48) return `fetched ${hours}h ago`;
  return `fetched ${Math.round(hours / 24)} days ago`;
}

type Props = {
  repoPath: string;
  status: RepoStatus;
  busy: boolean;
  /** Any staged or unstaged change in the worktree. */
  dirty: boolean;
  pullMode: PullMode;
  onPullMode: (m: PullMode) => void;
  pullLfs: boolean;
  onPullLfs: (v: boolean) => void;
  pullAutostash: boolean;
  onPullAutostash: (v: boolean) => void;
  onFetch: () => void;
  onPull: () => void;
  onPush: () => void;
  onBranches: () => void;
  onUndoCommit: () => void;
  onStash: () => void;
  onReset: () => void;
  onDiscardAll: () => void;
};

/** Where this branch stands, the two ways to move it, and the tidy-up strip. */
export function BranchCard({
  repoPath,
  status,
  busy,
  dirty,
  pullMode,
  onPullMode,
  pullLfs,
  onPullLfs,
  pullAutostash,
  onPullAutostash,
  onFetch,
  onPull,
  onPush,
  onBranches,
  onUndoCommit,
  onStash,
  onReset,
  onDiscardAll,
}: Props) {
  const pullBlocked = whyDisabled(
    pullMode,
    status.ahead,
    status.behind,
    dirty,
    pullMode === "rebase" && pullAutostash
  );
  const opTitle = status.operation ? `Finish or abort the ${status.operation.kind} first` : null;
  const uncommitted = status.staged_count + status.unstaged_count + status.untracked_count;
  const level = status.ahead === 0 && status.behind === 0;

  const undoTitle = status.unborn
    ? "There is no commit to undo yet"
    : opTitle
      ? opTitle
      : !status.can_amend
        ? `The last commit is already on ${status.upstream ?? "the remote"} — revert it from History instead`
        : null;
  const stashTitle = uncommitted === 0 ? "Nothing to stash" : null;
  const resetTitle = !status.upstream
    ? "This branch has no upstream"
    : level
      ? `Already level with ${status.upstream}`
      : opTitle;
  const discardTitle = uncommitted === 0 ? "Nothing to discard" : null;

  return (
    <Card>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0 space-y-2">
          <div className="flex items-center gap-2 flex-wrap">
            <GitBranch size={15} className="text-zinc-400 shrink-0" />
            {status.detached ? (
              <span className="font-medium text-amber-300">
                detached HEAD at {(status.head_oid ?? "").slice(0, 7) || "?"} — you&apos;re not on a branch
              </span>
            ) : (
              <span className="font-medium text-zinc-100">{status.branch ?? "no branch"}</span>
            )}
            {status.upstream ? (
              <span className="text-xs text-zinc-500">→ {status.upstream}</span>
            ) : (
              !status.detached && <span className="text-xs text-amber-400/90">not published yet</span>
            )}
            {status.ahead > 0 && (
              <span className="inline-flex items-center gap-0.5 text-xs text-emerald-400">
                <ArrowUp size={12} />
                {status.ahead}
              </span>
            )}
            {status.behind > 0 && (
              <span className="inline-flex items-center gap-0.5 text-xs text-sky-400">
                <ArrowDown size={12} />
                {status.behind}
              </span>
            )}
            <Button
              size="sm"
              variant="ghost"
              disabled={busy || !!status.operation}
              title={opTitle ?? (busy ? "Wait for the running operation" : "Switch, create, rename or delete local branches")}
              onClick={onBranches}
              className="px-2"
            >
              <GitBranch size={13} />
              Branches
            </Button>
            <span className="text-xs text-zinc-600">
              {agoLabel(status.last_fetch_secs)}
            </span>
          </div>
          <PullControl
            mode={pullMode}
            onMode={onPullMode}
            ahead={status.ahead}
            behind={status.behind}
            dirty={dirty}
            upstream={status.upstream}
            autostash={pullAutostash}
            onAutostash={onPullAutostash}
          />
          {status.uses_lfs && (
            <label
              className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer"
              title="A plain pull only downloads large files when this repository's LFS filters are set up — otherwise they arrive as pointer stubs"
            >
              <Checkbox
                checked={pullLfs}
                onChange={onPullLfs}
                className="mt-0.5"
                aria-label="Also download LFS files when pulling"
              />
              <span className="leading-relaxed">
                Also download Git LFS files
                <span className="text-zinc-500">
                  {" "}
                  — otherwise large files arrive as pointer stubs
                </span>
              </span>
            </label>
          )}
        </div>

        <div className="flex items-center gap-2 flex-wrap">
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || !repoPath}
            onClick={onFetch}
            className="min-w-[5.5rem]"
          >
            <RefreshCw size={13} />
            Fetch
          </Button>
          <Button
            size="sm"
            variant={status.behind > 0 ? "primary" : "secondary"}
            disabled={busy || !status.upstream || pullBlocked !== null}
            title={pullBlocked ?? undefined}
            onClick={onPull}
            className="min-w-[8.5rem]"
          >
            <Download size={13} />
            Pull ({pullMode === "ff-only" ? "fast-forward" : pullMode})
          </Button>
          <Button
            size="sm"
            variant={status.behind > 0 ? "secondary" : "primary"}
            disabled={busy || status.push.blocked || status.detached || status.unborn}
            title={status.push.blocked ? status.push.reason : undefined}
            onClick={onPush}
            className="min-w-[8.5rem]"
          >
            {status.push.blocked ? <Ban size={13} /> : <Upload size={13} />}
            {status.push.lock.locked
              ? "Push locked"
              : status.push.blocked
              ? "Push blocked"
              : status.upstream
                ? `Push${status.ahead ? ` (${status.ahead})` : ""}`
                : "Publish branch"}
          </Button>
        </div>
      </div>

      {/* Tidy up: the small, reversible-where-possible ways out of a mess. */}
      <div className="flex flex-wrap items-center gap-x-1 gap-y-1 border-t border-zinc-700/50 mt-3 pt-2">
        <span className="text-xs text-zinc-500 mr-1">Tidy up</span>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy || undoTitle !== null}
          title={undoTitle ?? "Takes the last commit apart: its changes come back staged, nothing is lost"}
          onClick={onUndoCommit}
        >
          <Undo2 size={13} />
          Undo last commit
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy || stashTitle !== null}
          title={stashTitle ?? "Sets your uncommitted changes aside so the tree is clean"}
          onClick={onStash}
        >
          <Archive size={13} />
          Stash changes…
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy || resetTitle !== null}
          title={
            resetTitle ??
            `Moves ${status.branch ?? "this branch"} to exactly where ${status.upstream} is; a backup branch keeps your commits`
          }
          onClick={onReset}
        >
          <RotateCcw size={13} />
          Reset to {status.upstream ?? "upstream"}…
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy || discardTitle !== null}
          title={discardTitle ?? "Throws away every uncommitted change — it asks first"}
          onClick={onDiscardAll}
          className="hover:text-red-400"
        >
          <Eraser size={13} />
          Discard everything…
        </Button>
      </div>
    </Card>
  );
}
