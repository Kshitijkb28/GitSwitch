import { useEffect, useState } from "react";
import { Button } from "../Button";
import { Input } from "../Input";
import { Modal } from "../Modal";

type Props = {
  open: boolean;
  onCancel: () => void;
  /**
   * Adds `url` as origin and starts the publish. Resolves to a message when
   * the address was refused: it is shown here and the dialog stays open, so
   * the person can correct the address.
   */
  onConfirm: (url: string) => Promise<string | null>;
};

/**
 * Publish, for a repository that is on no server yet: the address of an empty
 * repository to publish to. It is added as origin, then the branch is pushed
 * and tracked from then on. GitSwitch does not create the repository on
 * GitHub itself: the account, the name and who can see it are the person's to
 * choose there.
 */
export function AddRemoteModal({ open, onCancel, onConfirm }: Props) {
  const [url, setUrl] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) {
      setUrl("");
      setProblem(null);
      setBusy(false);
    }
  }, [open]);
  const address = url.trim();

  async function submit() {
    if (!address || busy) return;
    setBusy(true);
    setProblem(null);
    const p = await onConfirm(address);
    setBusy(false);
    if (p) setProblem(p);
  }

  return (
    <Modal open={open} onClose={busy ? () => undefined : onCancel} title="Publish to a new remote">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <p className="text-sm text-zinc-300">
          This repository isn't on a server yet. Create an empty repository on GitHub (or your own server), without a
          README, licence or .gitignore so it has no commits of its own, then paste its address here.
        </p>
        <Input
          aria-label="Remote address"
          placeholder="git@github.com:you/repo.git"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          autoFocus
          spellCheck={false}
          autoComplete="off"
        />
        {problem && <p className="text-xs text-red-300 break-words">{problem}</p>}
        <p className="text-xs text-zinc-500">
          It is added as <span className="font-mono">origin</span>, then this branch is published there and tracked from
          now on. An SSH address (git@github.com:…) signs in with this folder's profile key.
        </p>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" disabled={!address || busy}>
            {busy ? "Adding…" : "Add and publish"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
