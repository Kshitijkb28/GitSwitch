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

/**
 * Everything back to HEAD. New files stay unless asked; a stash is offered
 * instead of losing it all. The counts name only what the operation touches:
 * the parent's own tracked and conflicted files, and new files when ticked —
 * never a submodule row, which the backend leaves alone.
 */
export function DiscardAllModal({ open, status, onCancel, onConfirm }: Props) {
  const [includeUntracked, setIncludeUntracked] = useState(false);
  const [stashFirst, setStashFirst] = useState(false);
  useEffect(() => {
    if (open) {
      setIncludeUntracked(false);
      setStashFirst(false);
    }
  }, [open]);

  const files = status.entries.filter((e) => !e.is_submodule);
  const staged = files.filter((e) => e.kind === "tracked" && e.staged !== ".");
  const unstaged = files.filter((e) => e.kind === "tracked" && e.unstaged !== ".");
  const conflicted = files.filter((e) => e.kind === "conflicted");
  const a = staged.length;
  const b = unstaged.length;
  const x = conflicted.length;
  const c = status.untracked_count;
  // A file changed on both sides is one file.
  const tracked = new Set([...staged, ...unstaged, ...conflicted].map((e) => e.path)).size;
  const deleting = includeUntracked && c > 0;
  const n = tracked + (deleting ? c : 0);
  const submodules = status.entries
    .filter((e) => e.is_submodule && (e.kind === "conflicted" || e.staged !== "." || e.unstaged !== "."))
    .map((e) => e.path);
  const nothing = n === 0;
  const confirmTitle = nothing
    ? c > 0
      ? "Nothing to discard unless the new files are deleted too — tick the box above"
      : "Nothing to discard"
    : undefined;

  const plural = (k: number) => (k === 1 ? "" : "s");

  return (
    <Modal open={open} onClose={onCancel} title="Discard everything?">
      <div className="space-y-3">
        {nothing ? (
          <p className="text-sm text-zinc-300">
            There are no tracked changes to discard
            {c > 0 ? ` — only ${c} new file${plural(c)}, which stay${c === 1 ? "s" : ""} unless asked below` : ""}.
          </p>
        ) : (
          <p className="text-sm text-zinc-300">
            This throws away the changes in {n} file{plural(n)} ({a} staged, {b} not staged
            {x > 0 ? `, ${x} conflicted` : ""}){deleting ? ` and deletes ${c} new file${plural(c)}` : ""}.{" "}
            {!stashFirst && <span className="text-red-300">It cannot be undone.</span>}
          </p>
        )}
        {submodules.length > 0 && (
          <p className="text-xs text-zinc-500 break-words">
            Changes inside submodules ({submodules.join(", ")}) are never touched.
          </p>
        )}
        {c > 0 && (
          <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
            <Checkbox
              checked={includeUntracked}
              onChange={setIncludeUntracked}
              className="mt-0.5"
              aria-label="Also delete new files"
            />
            <span className="leading-relaxed">
              Also delete {c} new file{plural(c)}
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
          <Button
            variant="danger"
            size="sm"
            disabled={nothing}
            title={confirmTitle}
            onClick={() => onConfirm(deleting, stashFirst)}
          >
            Discard everything
          </Button>
        </div>
      </div>
    </Modal>
  );
}
