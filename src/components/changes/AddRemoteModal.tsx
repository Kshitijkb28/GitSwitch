import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "../Button";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { CreateRepoForm, repoNameFrom } from "./CreateRepoForm";
import { cachedAccounts, loadAccounts } from "./publishCache";
import { cleanError } from "../github/shared";
import * as api from "../../lib/api";
import { notifyRepoChange, sameRepo } from "../../lib/repoChanges";

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
  /**
   * "owner/name" GitHub lost (deleted here, or a push found nothing there):
   * make it again under that same name, so the folder's address stays as it
   * is and it publishes as before. The folder is always read first; there is
   * no address tab, and a remote already reached is never just pushed to. A
   * folder that turns out never to have reached its remote (or to have none)
   * gets the usual dialog, filled in.
   */
  again?: string | null;
  /**
   * The page's status of the folder. With no remote in it, the form is drawn
   * and usable at once (origin is what gets added); the folder is read again
   * only when a remote is there, to tell where a new repository would be
   * connected. The create command checks everything again before GitHub is
   * asked anything.
   */
  status?: api.RepoStatus | null;
  onCancel: () => void;
  /**
   * The address tab: checks the address, saves it and starts the publish.
   * Resolves to a problem when the address wasn't saved: it is shown here and
   * the dialog stays open. `allowExisting`: publish to a repository that
   * already has branches. `againChange`: made again, the folder turned out
   * never to have reached this remote: it is the one whose address changes.
   */
  onConfirm: (url: string, allowExisting: boolean, againChange?: { remote: string; from: string } | null) => Promise<AddressProblem | null>;
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
  again = null,
  status = null,
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
  /**
   * The folder's own answer (where a new repository would be connected), with
   * the folder — and the repository made again — it was read for. Cleared
   * when the dialog closes: an opening never builds its form from an earlier
   * opening's folder, nor from one made again that this one isn't.
   */
  const [readTarget, setReadTarget] = useState<{ for: string; context: api.PublishContext | null; error: string | null } | null>(null);
  const [createPrefill, setCreatePrefill] = useState<string | null>(null);
  /** The accounts as read for a folder; another folder's are never shown for this one. */
  const [loaded, setLoaded] = useState<{ for: string; accounts: api.PublishAccounts } | null>(null);
  const [accountsError, setAccountsError] = useState<string | null>(null);
  const [pushLogin, setPushLogin] = useState<string | null>(null);
  const readKey = `${repoPath ?? ""}\n${again ?? ""}`;
  const mine = open && readTarget?.for === readKey ? readTarget : null;
  const context = mine?.context ?? null;
  const contextError = mine?.error ?? null;
  // This folder's accounts: as read for it, else as kept from before (so the form is drawn with them at once).
  const accounts = repoPath ? (loaded?.for === repoPath ? loaded.accounts : cachedAccounts(repoPath)) : null;

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

  // The page's status as it was when the dialog opened: a refresh while it is
  // open (the create step itself saves origin) doesn't change what it does.
  // And the control that opened it, taken before the dialog's own fields take
  // the focus: it gets the focus back when the dialog closes, if still there.
  const openedWith = useRef<api.RepoStatus | null>(null);
  const opener = useRef<HTMLElement | null>(null);
  const wasOpen = useRef(false);
  if (open && !wasOpen.current) {
    openedWith.current = status;
    opener.current = document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : null;
  }
  wasOpen.current = open;
  useEffect(() => {
    if (open) return;
    // Closed: what was read for this opening goes with it.
    setReadTarget(null);
    setPushLogin(null);
    setAccountsError(null);
    const back = opener.current;
    opener.current = null;
    if (back)
      window.setTimeout(() => {
        if (back.isConnected) back.focus();
        // The control that opened it went with what the dialog closed on (a card
        // replaced, a note gone): the focus goes to the Repository page's tab the
        // person is on, rather than back to the start of the window.
        else document.querySelector<HTMLElement>('[role="tablist"][aria-label="Repository tabs"] [role="tab"][aria-selected="true"]')?.focus();
      }, 0);
  }, [open]);
  const seen = openedWith.current;
  // With no remote on the page, origin is what gets added: nothing to wait for.
  // Made again, the folder itself says where it publishes, whatever the page shows.
  const pageTarget: api.PublishTarget | null =
    !again && seen && seen.push.remotes.length === 0 ? { kind: "add", remote: "origin", url: null, repo: null } : null;
  const needContext = !!repoPath && (!!again || !pageTarget);

  // The accounts: at once when kept from before, and read again behind that
  // (local files only, so it is quick). Then who the folder pushes as, when
  // only github.com can say (its key, asked in the background).
  useEffect(() => {
    if (!open || !repoPath) return;
    let live = true;
    setAccountsError(null);
    setPushLogin(null);
    loadAccounts(repoPath).then(
      (a) => {
        if (!live) return;
        setLoaded({ for: repoPath, accounts: a });
        if (a.push_login_pending) {
          api.githubPublishPushLogin(repoPath).then(
            (l) => {
              if (live && l) setPushLogin(l);
            },
            () => undefined
          );
        }
      },
      (e) => {
        if (live) setAccountsError(cleanError(e));
      }
    );
    return () => {
      live = false;
    };
  }, [open, repoPath]);

  // Where a new repository would be connected, read when the page shows a
  // remote: a remote never reached gets the new address; one already reached
  // is simply pushed to — unless the repository is being made again, at the
  // address that remote already has.
  useEffect(() => {
    // Whatever was read before (another folder, or this one when it showed a
    // remote) is never this opening's answer: read again, or not used at all.
    setReadTarget(null);
    if (!open || !repoPath || !needContext) return;
    let live = true;
    const key = readKey;
    api.githubPublishContext(repoPath).then(
      (c) => {
        if (!live) return;
        if (c.target.kind === "none" && mode === "add" && onAlreadyConnected && !again) {
          onAlreadyConnected();
          return;
        }
        setReadTarget({ for: key, context: c, error: null });
      },
      (e) => {
        if (live) setReadTarget({ for: key, context: null, error: cleanError(e) });
      }
    );
    return () => {
      live = false;
    };
  }, [open, repoPath, needContext, again]); // eslint-disable-line react-hooks/exhaustive-deps

  // The form's starting point: the page's facts and the accounts first, the
  // folder's own answer once it is in.
  const known = context ?? accounts;
  const knownAccounts = known?.accounts ?? [];
  const pushedAs = pushLogin ? knownAccounts.find((a) => a.login.toLowerCase() === pushLogin.toLowerCase()) : undefined;
  const target: api.PublishTarget = context?.target ?? pageTarget ?? { kind: "none", remote: null, url: null, repo: null };
  // Made again: the remote the push goes to, reached before, still names the
  // repository GitHub lost. "change" and "add" are their own flows; a remote
  // naming another repository stays "none", and the form says so.
  const formTarget: api.PublishTarget =
    again && target.kind === "none" && target.repo && sameRepo(target.repo, again) ? { ...target, kind: "again" } : target;
  // Made again where the address points, or about to be (the folder is being
  // read): said in the title, and no tabs. A folder that turns out never to
  // have reached its remote, or to have none, gets the usual dialog.
  const makingAgain = !!again && formTarget.kind !== "add" && formTarget.kind !== "change";
  /** Made again, but the folder never reached its remote: the address tab changes that one. */
  const againChange = again && formTarget.kind === "change" && formTarget.remote && formTarget.url ? { remote: formTarget.remote, from: formTarget.url } : null;
  const formContext: api.PublishContext | null = repoPath
    ? {
        accounts: knownAccounts,
        suggested: pushedAs?.account ?? known?.suggested ?? null,
        push_login: pushLogin ?? known?.push_login ?? null,
        target: formTarget,
        default_name: context?.default_name ?? (repoNameFrom(folderName(repoPath)) || "repository"),
        branch: context?.branch ?? seen?.branch ?? null,
        blocked: context?.blocked ?? null,
      }
    : null;
  const accountsPending = !known && !accountsError;
  const targetPending = needContext && !context && !contextError;

  const address = url.trim();
  const changing = mode === "change" || !!againChange;
  const anyway = (problem?.existingBranches?.length ?? 0) > 0;
  const working = busy || createBusy;

  async function submit(allowExisting = false) {
    if (!address || busy) return;
    setBusy(true);
    setProblem(null);
    const p = await onConfirm(address, allowExisting, againChange);
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
    // It is on GitHub now, connected or not: the pages that list repositories,
    // and a GitHub tab that said it was deleted, hear of it.
    notifyRepoChange({ kind: "created", fullName: p.full_name, private: p.private });
    // It exists on GitHub but isn't connected: finish it here, by its address.
    // Made again where the address points there is no address tab: the page says what stands in the way.
    if (p.saved === "no" && !makingAgain) {
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
    <Modal open={open} onClose={working ? () => undefined : onCancel} title={makingAgain ? "Create it again on GitHub" : "Publish this repository"}>
      <div className="space-y-4">
        {/* Made again where the address points, it is the folder's own: there is nothing to paste. */}
        {repoPath && !makingAgain && (
          <div role="tablist" aria-label="Where to publish" className="flex gap-1 p-1 rounded-lg bg-zinc-800/60 w-fit">
            {tabButton("create", "Create on GitHub")}
            {tabButton("address", "Use an address")}
          </div>
        )}
        {tab === "create" && repoPath && formContext ? (
          <CreateRepoForm
            key={again ?? createPrefill ?? prefill ?? ""}
            repoPath={repoPath}
            context={formContext}
            accountsPending={accountsPending}
            targetPending={targetPending}
            problem={contextError ?? accountsError}
            prefill={again ?? createPrefill ?? prefill}
            again={again}
            onCreated={created}
            onUseAddress={(u) => {
              setTab("address");
              setUrl(u);
              setProblem(null);
            }}
            onBusy={setCreateBusy}
            onCancel={onCancel}
          />
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

/** The last part of a folder's path. */
function folderName(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}
