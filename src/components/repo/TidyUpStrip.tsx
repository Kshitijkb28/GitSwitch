import { Undo2, Archive, RotateCcw, Eraser } from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import { Checkbox } from "../Checkbox";
import { PullControl } from "../changes/PullControl";
import type { PullMode, RepoStatus } from "../../lib/api";

type Props = {
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
  onUndoCommit: () => void;
  onStash: () => void;
  onReset: () => void;
  onDiscardAll: () => void;
};

/**
 * What used to be the lower half of the branch card: how a pull should
 * behave, and the small, reversible-where-possible ways out of a mess. The
 * branch facts and the Fetch / Pull / Push buttons now live on the status strip
 * above the tabs, where every tab can see them.
 */
export function TidyUpStrip({
  status,
  busy,
  dirty,
  pullMode,
  onPullMode,
  pullLfs,
  onPullLfs,
  pullAutostash,
  onPullAutostash,
  onUndoCommit,
  onStash,
  onReset,
  onDiscardAll,
}: Props) {
  const opTitle = status.operation ? `Finish or abort the ${status.operation.kind} first` : null;
  const uncommitted = status.staged_count + status.unstaged_count + status.untracked_count;
  // Discard everything also clears conflicts left behind by a stash pop — the
  // backend allows exactly that when no operation is in progress.
  const discardable = uncommitted + status.conflicted_count;
  const level = status.ahead === 0 && status.behind === 0;

  // `can_amend` is false when ANY remote-tracking ref holds HEAD — not
  // necessarily the upstream — so the tooltip must not name one.
  const undoTitle = status.unborn
    ? "There is no commit to undo yet"
    : opTitle
      ? opTitle
      : !status.can_amend
        ? "The last commit is already on a remote branch — revert it from History instead"
        : null;
  const stashTitle = uncommitted === 0 ? "Nothing to stash" : null;
  const resetTitle = !status.upstream
    ? "This branch has no upstream"
    : level
      ? `Already level with ${status.upstream}`
      : opTitle;
  const discardTitle = discardable === 0 ? "Nothing to discard" : null;

  return (
    <Card>
      <div className="space-y-2">
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
              <span className="text-zinc-500"> — otherwise large files arrive as pointer stubs</span>
            </span>
          </label>
        )}
      </div>

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
            `Moves ${status.branch ?? "this branch"} to exactly where ${status.upstream} is; a backup branch keeps your commits. To take the latest and keep your commits, use Pull instead.`
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
