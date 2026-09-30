import { useEffect, useState } from "react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { usePersistedState } from "../../lib/persist";
import type { RepoStatus } from "../../lib/api";

type Props = {
  open: boolean;
  status: RepoStatus;
  onCancel: () => void;
  onConfirm: (message: string | null, includeUntracked: boolean) => void;
};

/** `git stash push`: the form is the whole step, so there is no second confirmation. */
export function StashModal({ open, status, onCancel, onConfirm }: Props) {
  const [message, setMessage] = useState("");
  const [includeUntracked, setIncludeUntracked] = usePersistedState("changes.stashUntracked", true);
  useEffect(() => {
    if (open) setMessage("");
  }, [open]);

  const a = status.staged_count;
  const b = status.unstaged_count;
  const c = status.untracked_count;
  const n = a + b + c;
  const withUntracked = c > 0 && includeUntracked;

  const submit = () => onConfirm(message.trim() || null, withUntracked);

  return (
    <Modal open={open} onClose={onCancel} title="Stash changes">
      <div className="space-y-3">
        <p className="text-sm text-zinc-300">
          Sets aside your {n} uncommitted change{n === 1 ? "" : "s"} ({a} staged, {b} not staged, {c} new) so the
          tree is clean. They come back with Apply or Pop.
        </p>
        <Input
          aria-label="Stash message"
          placeholder="What is this work? (optional)"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
        {c > 0 && (
          <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
            <Checkbox
              checked={includeUntracked}
              onChange={setIncludeUntracked}
              className="mt-0.5"
              aria-label="Include untracked files"
            />
            <span className="leading-relaxed">
              Include {c} untracked file{c === 1 ? "" : "s"}
              <span className="text-zinc-500"> — otherwise new files stay in the tree</span>
            </span>
          </label>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" size="sm" onClick={onCancel}>
            Not now
          </Button>
          <Button size="sm" onClick={submit}>
            Stash
          </Button>
        </div>
      </div>
    </Modal>
  );
}
