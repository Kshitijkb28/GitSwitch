import { useEffect, useRef, useState } from "react";
import { AlertTriangle, Eye, Globe, Loader2, Lock, Trash2 } from "lucide-react";
import * as api from "../../lib/api";
import type { Deleted } from "../../lib/repoChanges";
import { Button } from "../Button";
import { RefreshButton } from "../RefreshButton";
import { DeleteRepoModal } from "./DeleteRepoModal";
import { VisibilityModal, VisibilityNow } from "./VisibilityModal";
import { cleanError } from "./shared";
import { deleteBlock, loginOf, visibilityBlock, wantFor, type Want } from "./repoAdmin";

type Props = {
  account: string;
  owner: string;
  name: string;
  /** The folder open on the Repository page: the delete dialog checks it for commits GitHub has and it lacks. */
  repoPath: string;
  reloadKey?: number;
  /** What GitHub said about who can see it: read here, or changed here. */
  onVisibility: (visibility: api.RepoFacts["visibility"]) => void;
  /** Deleted on GitHub: its name, GitHub's answer, and where it can be restored. */
  onDeleted: (deleted: Deleted) => void;
};

/**
 * The open repository's settings on GitHub that GitSwitch can change: who can
 * see it, and — in the danger zone — deleting it. Read as the account acting,
 * so a button that can't work for it is greyed with the reason.
 */
export function RepoSettings({ account, owner, name, repoPath, reloadKey = 0, onVisibility, onDeleted }: Props) {
  const [facts, setFacts] = useState<api.RepoFacts | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"visibility" | "delete" | null>(null);
  /** The step the card's button offered when it was pressed: the dialog does that one, or says why not. */
  const [picked, setPicked] = useState<Want>("private");
  const ask = useRef(0);
  /** The card's button that opened the dialog: the focus goes back to it when the dialog closes. */
  const opener = useRef<HTMLButtonElement | null>(null);
  const login = loginOf(account);

  /** Opens a dialog from the button pressed. */
  function openDialog(which: "visibility" | "delete", from: HTMLButtonElement) {
    opener.current = from;
    setDialog(which);
  }

  /** A dialog closed: the focus goes back to the button that opened it (gone with the cards after a delete). */
  function closeDialog() {
    setDialog(null);
    window.setTimeout(() => {
      const b = opener.current;
      if (b?.isConnected && !b.disabled) b.focus();
    }, 0);
  }

  // Another account or repository: a dialog opened for the one before is closed.
  useEffect(() => {
    setDialog(null);
  }, [account, owner, name]);

  async function load() {
    const id = ++ask.current;
    setLoading(true);
    setError(null);
    try {
      const f = await api.githubRepoFacts(account, owner, name, null);
      if (id !== ask.current) return;
      setFacts(f);
      onVisibility(f.visibility);
    } catch (e) {
      if (id === ask.current) {
        setFacts(null);
        setError(cleanError(e));
      }
    } finally {
      if (id === ask.current) setLoading(false);
    }
  }

  useEffect(() => {
    setFacts(null);
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account, owner, name, reloadKey]);

  const want = facts ? wantFor(facts.visibility) : "private";
  const visBlock = facts ? visibilityBlock(facts, login) : null;
  const delBlock = facts ? deleteBlock(facts.admin, login) : null;

  return (
    <div className="space-y-4" data-repo-settings>
      <div className="flex items-center justify-between gap-2 flex-wrap">
        <p className="text-xs text-zinc-500">
          As <span className="text-zinc-300">@{login}</span> on GitHub. Changes here happen on GitHub at once.
        </p>
        <RefreshButton onClick={() => void load()} busy={loading} title="Read the repository's settings again" />
      </div>

      {loading && !facts && (
        <p role="status" className="flex items-center gap-2 text-sm text-zinc-400">
          <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading {owner}/{name} from GitHub…
        </p>
      )}
      {error && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
          {error}
        </p>
      )}

      {facts && (
        <>
          <section aria-label="Who can see it" className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-4 space-y-3" data-settings-card="visibility">
            <h3 className="text-sm font-medium text-zinc-200 flex items-center gap-2">
              <Eye size={15} className="text-zinc-400" />
              Who can see it
            </h3>
            <div className="flex items-start gap-3 flex-wrap">
              <div className="flex-1 min-w-[14rem] text-sm">
                <VisibilityNow facts={facts} />
              </div>
              <Button
                variant="secondary"
                size="sm"
                onClick={(e) => {
                  setPicked(want);
                  openDialog("visibility", e.currentTarget);
                }}
                disabled={!!visBlock}
                title={visBlock ?? undefined}
              >
                {want === "public" ? <Globe size={14} /> : <Lock size={14} />}
                {want === "public" ? "Make public…" : "Make private…"}
              </Button>
            </div>
            {visBlock && <p className="text-xs text-amber-300/90">{visBlock}</p>}
          </section>

          <section aria-label="Danger zone" className="rounded-xl border border-red-500/40 bg-red-500/[0.04] p-4 space-y-3" data-settings-card="danger">
            <h3 className="text-sm font-medium text-red-300 flex items-center gap-2">
              <AlertTriangle size={15} />
              Danger zone
            </h3>
            <div className="flex items-start gap-3 flex-wrap">
              <div className="flex-1 min-w-[14rem]">
                <p className="text-sm text-zinc-200">Delete this repository on GitHub</p>
                <p className="text-xs text-zinc-400 mt-0.5">Its code, issues, pull requests and settings go with it. The folder on this computer is not touched.</p>
              </div>
              <Button variant="danger" size="sm" onClick={(e) => openDialog("delete", e.currentTarget)} disabled={!!delBlock} title={delBlock ?? undefined}>
                <Trash2 size={14} />
                Delete on GitHub…
              </Button>
            </div>
            {delBlock && <p className="text-xs text-amber-300/90">{delBlock}</p>}
          </section>
        </>
      )}

      <VisibilityModal
        open={dialog === "visibility"}
        account={account}
        owner={owner}
        name={name}
        want={picked}
        onClose={closeDialog}
        onChanged={(f) => {
          setFacts(f);
          onVisibility(f.visibility);
        }}
      />
      <DeleteRepoModal open={dialog === "delete"} account={account} owner={owner} name={name} repoPath={repoPath} onClose={closeDialog} onDeleted={onDeleted} />
    </div>
  );
}
