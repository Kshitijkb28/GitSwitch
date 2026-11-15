import { Button } from "../Button";
import { Modal } from "../Modal";
import type { ChangeEntry } from "../../lib/api";
import type { DiscardVariant } from "./TreeLists";

type Props = {
  open: boolean;
  paths: string[];
  /** The entries of the repository the paths belong to, to say which get deleted from disk. */
  entries: ChangeEntry[];
  /** "changes" throws away edits; "untracked" deletes new files (Delete all untracked). */
  variant: DiscardVariant;
  onCancel: () => void;
  onConfirm: () => void;
};

/** The one confirmation for anything that removes work without a way back. */
export function DiscardConfirmModal({ open, paths, entries, variant, onCancel, onConfirm }: Props) {
  const found = paths.map((p) => entries.find((e) => e.path === p));
  // Discarding goes back to the last commit, which has no file at a new path:
  // a staged rename's or copy's new name is deleted from disk like a new file.
  const willDelete = found.filter((e) => e && (e.kind === "untracked" || e.staged === "A" || e.staged === "R" || e.staged === "C")).length;
  const renamed = found.filter((e) => e && e.staged === "R").length;
  const n = paths.length;

  return (
    <Modal
      open={open}
      onClose={onCancel}
      title={variant === "untracked" ? "Delete these new files?" : "Discard these changes?"}
    >
      <div className="space-y-3">
        {variant === "untracked" ? (
          <>
            <p className="text-sm text-zinc-300">
              {n === 1 ? "This deletes 1 untracked file from disk." : `This deletes ${n} untracked files from disk.`}{" "}
              <span className="text-red-300">It cannot be undone.</span>
            </p>
            <p className="text-xs text-zinc-500">Ignored files are never touched.</p>
          </>
        ) : (
          <>
            <p className="text-sm text-zinc-300">
              {n === 1
                ? "This throws away the changes in 1 file."
                : `This throws away the changes in ${n} files.`}{" "}
              <span className="text-red-300">It cannot be undone.</span>
            </p>
            {willDelete > 0 && (
              <p className="text-sm text-amber-300">
                {willDelete} of them {willDelete === 1 ? "is a new file and will be" : "are new files and will be"}{" "}
                deleted from disk.
              </p>
            )}
            {renamed > 0 && (
              <p className="text-xs text-zinc-400">
                {renamed === 1
                  ? "That counts a renamed file under its new name; it goes back to its old name, as in the last commit."
                  : `That counts ${renamed} renamed files under their new names; they go back to their old names, as in the last commit.`}
              </p>
            )}
          </>
        )}
        <ul className="max-h-40 overflow-y-auto text-xs font-mono text-zinc-400 space-y-0.5">
          {paths.slice(0, 50).map((p) => (
            <li key={p} className="truncate">
              {p}
            </li>
          ))}
          {n > 50 && <li className="text-zinc-500">…and {n - 50} more</li>}
        </ul>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" size="sm" onClick={onCancel}>
            Keep them
          </Button>
          <Button variant="danger" size="sm" onClick={onConfirm}>
            {variant === "untracked" ? "Delete" : "Discard"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
