import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "../Button";
import { Input } from "../Input";
import { Modal } from "../Modal";

/** Why the address wasn't saved, and what can be done about it from here. */
export type AddressProblem = {
  text: string;
  /** GitHub's new-repository form, filled in: the repository doesn't exist yet. */
  createUrl?: string | null;
  /** The branches a repository that isn't empty already has: "Publish there anyway" is offered. */
  existingBranches?: string[];
};

type Props = {
  open: boolean;
  /** "change": a new address for a remote that was never reached (after a push found nothing there). */
  mode?: "add" | "change";
  /** The address to start from: the remote's current one, when changing it. */
  initialUrl?: string;
  onCancel: () => void;
  /**
   * Checks the address, saves it and starts the publish. Resolves to a problem
   * when the address wasn't saved: it is shown here and the dialog stays open,
   * so the person can act on it. `allowExisting`: publish to a repository that
   * already has branches.
   */
  onConfirm: (url: string, allowExisting: boolean) => Promise<AddressProblem | null>;
};

/**
 * Publish, for a repository that is on no server yet (or whose address had no
 * repository behind it): the address of an empty repository to publish to.
 * The backend checks it before saving anything; then the branch is pushed and
 * tracked from then on. GitSwitch doesn't create the repository on GitHub
 * itself: the account, the name and who can see it are the person's to choose
 * there, on the form "Create it on GitHub" opens filled in.
 */
export function AddRemoteModal({ open, mode = "add", initialUrl = "", onCancel, onConfirm }: Props) {
  const [url, setUrl] = useState("");
  const [problem, setProblem] = useState<AddressProblem | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) {
      setUrl(initialUrl);
      setProblem(null);
      setBusy(false);
    }
  }, [open, initialUrl]);
  const address = url.trim();
  const changing = mode === "change";
  const anyway = (problem?.existingBranches?.length ?? 0) > 0;

  async function submit(allowExisting = false) {
    if (!address || busy) return;
    setBusy(true);
    setProblem(null);
    const p = await onConfirm(address, allowExisting);
    setBusy(false);
    if (p) setProblem(p);
  }

  function createOnGitHub(link: string) {
    openUrl(link).catch(() =>
      setProblem((p) => (p ? { ...p, text: `${p.text} (Couldn't open the browser: open ${link} yourself.)` } : p))
    );
  }

  return (
    <Modal
      open={open}
      onClose={busy ? () => undefined : onCancel}
      title={changing ? "Change the remote address" : "Publish to a new remote"}
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <p className="text-sm text-zinc-300">
          {changing
            ? "Nothing has been fetched from or pushed to this address yet, so it can still be corrected. Paste the address of the repository to publish to: an empty one, without a README, licence or .gitignore."
            : "This repository isn't on a server yet. Create an empty repository on GitHub (or your own server), without a README, licence or .gitignore so it has no commits of its own, then paste its address here."}
        </p>
        <Input
          aria-label="Remote address"
          placeholder="git@github.com:you/repo.git"
          value={url}
          onChange={(e) => {
            setUrl(e.target.value);
            setProblem(null);
          }}
          autoFocus
          spellCheck={false}
          autoComplete="off"
        />
        {problem && (
          <div className="space-y-2">
            <p className="text-xs text-red-300 break-words">{problem.text}</p>
            {(problem.createUrl || anyway) && (
              <div className="flex flex-wrap gap-2">
                {problem.createUrl && (
                  <Button type="button" size="sm" variant="secondary" onClick={() => createOnGitHub(problem.createUrl!)}>
                    Create it on GitHub
                  </Button>
                )}
                {anyway && (
                  <Button type="button" size="sm" variant="secondary" onClick={() => void submit(true)} disabled={busy}>
                    Publish there anyway
                  </Button>
                )}
              </div>
            )}
          </div>
        )}
        <p className="text-xs text-zinc-500">
          It is checked first: nothing is saved unless there is a repository at that address. Then it is{" "}
          {changing ? (
            "saved as the new address"
          ) : (
            <>
              added as <span className="font-mono">origin</span>
            </>
          )}
          , and this branch is published there and tracked from now on. An SSH address (git@github.com:…) signs in with
          this folder's profile key.
        </p>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" disabled={!address || busy}>
            {busy ? "Checking…" : changing ? "Change and publish" : "Add and publish"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
