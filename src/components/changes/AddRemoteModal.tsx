import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Loader2 } from "lucide-react";
import { Button } from "../Button";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { CreateRepoForm } from "./CreateRepoForm";
import { cleanError } from "../github/shared";
import * as api from "../../lib/api";

/** Why the address wasn't saved, and what can be done about it from here. */
export type AddressProblem = {
  text: string;
  /** GitHub's new-repository page, filled in: the fallback when the app can't create it. */
  createUrl?: string | null;
  /** "owner/name" of a repository that doesn't exist yet: "Create it on GitHub" fills the form with it. */
  repo?: string | null;
  /** The branches a repository that isn't empty already has: "Publish there anyway" is offered. */
  existingBranches?: string[];
};

type Tab = "create" | "address";

type Props = {
  open: boolean;
  /** "change": the address tab gives a new address to a remote that was never reached. */
  mode?: "add" | "change";
  /** What the address tab starts from: the remote's current address, when changing it. */
  initialUrl?: string;
  /** The tab that opens. */
  initialTab?: Tab;
  /** The folder. Without it, there is no "Create on GitHub" tab. */
  repoPath?: string;
  /** "owner/name" the create form starts from. */
  prefill?: string | null;
  onCancel: () => void;
  /**
   * The address tab: checks the address, saves it and starts the publish.
   * Resolves to a problem when the address wasn't saved: it is shown here and
   * the dialog stays open. `allowExisting`: publish to a repository that
   * already has branches.
   */
  onConfirm: (url: string, allowExisting: boolean) => Promise<AddressProblem | null>;
  /** Created on GitHub and connected: the caller publishes (or shows what still stands in the way). */
  onCreated?: (published: api.Published) => void;
  /** The folder publishes to a remote already reached after all (the page was behind): just push. */
  onAlreadyConnected?: () => void;
};

/**
 * Publish this repository: create it on GitHub without leaving GitSwitch (the
 * default tab), or use the address of an empty repository (another server, or
 * one made already). Nothing is saved before GitHub or the server confirms a
 * repository to publish to.
 */
export function AddRemoteModal({
  open,
  mode = "add",
  initialUrl = "",
  initialTab,
  repoPath,
  prefill = null,
  onCancel,
  onConfirm,
  onCreated,
  onAlreadyConnected,
}: Props) {
  const [tab, setTab] = useState<Tab>("create");
  const [url, setUrl] = useState("");
  const [problem, setProblem] = useState<AddressProblem | null>(null);
  const [busy, setBusy] = useState(false);
  const [createBusy, setCreateBusy] = useState(false);
  const [context, setContext] = useState<api.PublishContext | null>(null);
  const [contextError, setContextError] = useState<string | null>(null);
  const [createPrefill, setCreatePrefill] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setTab(initialTab ?? (repoPath ? "create" : "address"));
      setUrl(initialUrl);
      setProblem(null);
      setBusy(false);
      setCreateBusy(false);
      setCreatePrefill(null);
    }
  }, [open, initialUrl, initialTab, repoPath]);

  // What the create form starts from, read on every opening: the page's
  // status may be older than the folder.
  useEffect(() => {
    if (!open || !repoPath) return;
    let live = true;
    setContext(null);
    setContextError(null);
    api.githubPublishContext(repoPath).then(
      (c) => {
        if (!live) return;
        if (c.target.kind === "none" && mode === "add" && onAlreadyConnected) {
          onAlreadyConnected();
          return;
        }
        setContext(c);
      },
      (e) => {
        if (live) setContextError(cleanError(e));
      }
    );
    return () => {
      live = false;
    };
  }, [open, repoPath]); // eslint-disable-line react-hooks/exhaustive-deps

  const address = url.trim();
  const changing = mode === "change";
  const anyway = (problem?.existingBranches?.length ?? 0) > 0;
  const working = busy || createBusy;

  async function submit(allowExisting = false) {
    if (!address || busy) return;
    setBusy(true);
    setProblem(null);
    const p = await onConfirm(address, allowExisting);
    setBusy(false);
    if (p) setProblem(p);
  }

  /** "Create it on GitHub" from the address tab: the form, filled in; the browser only without a folder. */
  function createIt(p: AddressProblem) {
    if (repoPath && p.repo) {
      setCreatePrefill(p.repo);
      setTab("create");
      return;
    }
    const link = p.createUrl;
    if (link) {
      openUrl(link).catch(() =>
        setProblem((cur) => (cur ? { ...cur, text: `${cur.text} (Couldn't open the browser: open ${link} yourself.)` } : cur))
      );
    }
  }

  function created(p: api.Published) {
    if (p.saved === "no") {
      // It exists on GitHub but isn't connected: finish it here, by its address.
      setTab("address");
      setUrl(p.url);
      setProblem({ text: p.problem });
      return;
    }
    onCreated?.(p);
  }

  const tabButton = (t: Tab, label: string) => (
    <button
      type="button"
      role="tab"
      aria-selected={tab === t}
      disabled={working}
      onClick={() => setTab(t)}
      className={`px-3 py-1.5 rounded-md text-sm transition-colors cursor-pointer disabled:cursor-default ${
        tab === t ? "bg-zinc-700 text-zinc-100" : "text-zinc-400 hover:text-zinc-200"
      }`}
    >
      {label}
    </button>
  );

  return (
    <Modal open={open} onClose={working ? () => undefined : onCancel} title="Publish this repository">
      <div className="space-y-4">
        {repoPath && (
          <div role="tablist" aria-label="Where to publish" className="flex gap-1 p-1 rounded-lg bg-zinc-800/60 w-fit">
            {tabButton("create", "Create on GitHub")}
            {tabButton("address", "Use an address")}
          </div>
        )}
        {tab === "create" && repoPath ? (
          context ? (
            <CreateRepoForm
              key={createPrefill ?? prefill ?? ""}
              repoPath={repoPath}
              context={context}
              prefill={createPrefill ?? prefill}
              onCreated={created}
              onUseAddress={(u) => {
                setTab("address");
                setUrl(u);
                setProblem(null);
              }}
              onBusy={setCreateBusy}
            />
          ) : contextError ? (
            <p role="alert" className="text-xs text-red-300 break-words">
              {contextError}
            </p>
          ) : (
            <p role="status" className="flex items-center gap-2 text-sm text-zinc-400">
              <Loader2 size={14} className="animate-spin" /> Reading this folder's accounts…
            </p>
          )
        ) : (
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
                : "Paste the address of an empty repository (on GitHub or your own server), one without a README, licence or .gitignore, so it has no commits of its own."}
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
                <p role="alert" className="text-xs text-red-300 break-words">
                  {problem.text}
                </p>
                {(problem.repo || problem.createUrl || anyway) && (
                  <div className="flex flex-wrap gap-2">
                    {(problem.repo || problem.createUrl) && (
                      <Button type="button" size="sm" variant="secondary" onClick={() => createIt(problem)}>
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
        )}
      </div>
    </Modal>
  );
}
