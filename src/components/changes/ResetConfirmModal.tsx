import { useEffect, useState } from "react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Modal } from "../Modal";
import type { RepoStatus } from "../../lib/api";

type Props = {
  open: boolean;
  status: RepoStatus;
  onCancel: () => void;
  onConfirm: (stashFirst: boolean) => void;
};

/**
 * "Reset to <upstream>" is `git reset --hard @{u}` with a backup branch first.
 * The two things it can lose — unpushed commits and uncommitted changes — are
 * each named with their count, and the second one is stashed unless the user
 * says otherwise.
 */
export function ResetConfirmModal({ open, status, onCancel, onConfirm }: Props) {
  const [stashFirst, setStashFirst] = useState(true);
  // A fresh decision every time the dialog opens.
  useEffect(() => {
    if (open) setStashFirst(true);
  }, [open]);

  const upstream = status.upstream ?? "upstream";
  const branch = status.branch ?? "this branch";
  const dirtyCount = status.staged_count + status.unstaged_count;
  const ahead = status.ahead;

  return (
    <Modal open={open} onClose={onCancel} title={`Reset to ${upstream}?`}>
      <div className="space-y-3">
        <p className="text-sm text-zinc-300">
          Moves <span className="font-mono text-zinc-100">{branch}</span> to where{" "}
          <span className="font-mono text-zinc-100">{upstream}</span> is.{" "}
          {ahead > 0
            ? `Your ${ahead} unpushed commit${ahead === 1 ? "" : "s"} leave${ahead === 1 ? "s" : ""} the branch.`
            : "Nothing of yours leaves the branch; it just moves forward."}
        </p>
        {ahead > 0 && (
          <p className="text-xs text-zinc-400">
            A backup branch (gitswitch-before-reset-&lt;time&gt;) keeps them; the undo command is shown
            afterwards.
          </p>
        )}
        {dirtyCount > 0 && (
          <div className="space-y-2">
            <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
              <Checkbox
                checked={stashFirst}
                onChange={setStashFirst}
                className="mt-0.5"
                aria-label="Stash uncommitted changes first"
              />
              <span className="leading-relaxed">
                Stash my {dirtyCount} uncommitted change{dirtyCount === 1 ? "" : "s"} first
                <span className="text-zinc-500"> — they come back with Apply or Pop</span>
              </span>
            </label>
            {!stashFirst && (
              <p className="text-xs text-red-300">
                Without stashing, your {dirtyCount} uncommitted change{dirtyCount === 1 ? "" : "s"}{" "}
                {dirtyCount === 1 ? "is" : "are"} thrown away. That cannot be undone.
              </p>
            )}
          </div>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" size="sm" onClick={onCancel}>
            Keep things as they are
          </Button>
          <Button variant="danger" size="sm" onClick={() => onConfirm(dirtyCount > 0 && stashFirst)}>
            Reset
          </Button>
        </div>
      </div>
    </Modal>
  );
}
