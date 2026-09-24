import { useEffect, useState } from "react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Modal } from "../Modal";
import type { RepoStatus } from "../../lib/api";

type Props = {
  open: boolean;
  status: RepoStatus;
  onCancel: () => void;
  onConfirm: (includeUntracked: boolean, stashFirst: boolean) => void;
};

/** Everything back to HEAD. New files stay unless asked; a stash is offered instead of losing it all. */
export function DiscardAllModal({ open, status, onCancel, onConfirm }: Props) {
  const [includeUntracked, setIncludeUntracked] = useState(false);
  const [stashFirst, setStashFirst] = useState(false);
  useEffect(() => {
    if (open) {
      setIncludeUntracked(false);
      setStashFirst(false);
    }
  }, [open]);

  const n = status.entries.length;
  const a = status.staged_count;
  const b = status.unstaged_count;
  const c = status.untracked_count;

  return (
    <Modal open={open} onClose={onCancel} title="Discard everything?">
      <div className="space-y-3">
        <p className="text-sm text-zinc-300">
          This throws away the changes in {n} file{n === 1 ? "" : "s"} ({a} staged, {b} not staged, {c} new).{" "}
          {!stashFirst && <span className="text-red-300">It cannot be undone.</span>}
        </p>
        {c > 0 && (
          <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
            <Checkbox
              checked={includeUntracked}
              onChange={setIncludeUntracked}
              className="mt-0.5"
              aria-label="Also delete new files"
            />
            <span className="leading-relaxed">
              Also delete {c} new file{c === 1 ? "" : "s"}
              <span className="text-zinc-500"> — ignored files are never touched</span>
            </span>
          </label>
        )}
        <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
          <Checkbox
            checked={stashFirst}
            onChange={setStashFirst}
            className="mt-0.5"
            aria-label="Stash them first"
          />
          <span className="leading-relaxed">
            Stash them first instead of losing them
            <span className="text-zinc-500"> — they come back with Apply or Pop</span>
          </span>
        </label>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" size="sm" onClick={onCancel}>
            Keep them
          </Button>
          <Button variant="danger" size="sm" onClick={() => onConfirm(c > 0 && includeUntracked, stashFirst)}>
            Discard everything
          </Button>
        </div>
      </div>
    </Modal>
  );
}
