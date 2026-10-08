import { useEffect, useRef, useState } from "react";
import { ArrowDownUp, Loader2, Play, RefreshCw, X } from "lucide-react";
import { Button } from "../Button";
import { usePersistedState } from "../../lib/persist";
import { assess, clearMoved, useUpdatePlan, whyNoPreview } from "../../lib/updatePlan";
import { SyncPlanView } from "./SyncPlanView";
import type { RepoStatus } from "../../lib/api";

type Props = {
  repoPath: string;
  status: RepoStatus;
  busy: boolean;
  onClose: () => void;
  /** Runs the plan as a long job; the result arrives through the page's result card. */
  onRun: (stash: boolean, bundles: boolean, fingerprint: [string, string][]) => void;
  /** A remedy button in the plan; resolves when the operation it ran has finished. */
  onRemedy: (code: string, sub?: string) => Promise<void>;
};

/**
 * "The latest from origin/main, your work on top" — read before it runs.
 * Opening it assesses (a fetch, then a plan that changes nothing); Update now
 * runs exactly that plan. Nothing here pushes.
 */
export function UpdateDrawer({ repoPath, status, busy, onClose, onRun, onRemedy }: Props) {
  const [stash, setStash] = usePersistedState("changes.syncStash", true);
  const [bundles, setBundles] = usePersistedState("changes.syncBundles", false);
  const st = useUpdatePlan(repoPath);
  const blocked = whyNoPreview(status);
  // git spells an upstream "remotes/origin/main" when a local branch shares
  // its short name; the words here use the plain name, the command line git's
  // own unambiguous one.
  const rawUpstream = status.upstream ?? "upstream";
  const upstream = rawUpstream.replace(/^(refs\/)?remotes\//, "");
  const [replanWhenFresh, setReplanWhenFresh] = useState(false);
  const statusRef = useRef(status);
  statusRef.current = status;

  // Opening without a plan assesses once (a fetch); a refused run re-assesses too.
  useEffect(() => {
    if (blocked || st.assessing) return;
    if (!st.plan || (st.moved && !st.movedPlanned)) void assess(repoPath, statusRef.current, stash, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repoPath, st.moved]);

  // After a remedy ran, the repository is re-read; plan again on the fresh
  // facts, without a second fetch.
  useEffect(() => {
    if (!replanWhenFresh || st.assessing) return;
    setReplanWhenFresh(false);
    void assess(repoPath, status, stash, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [replanWhenFresh, status.head_oid, status.stash_count, status.untracked_count]);

  // Closing (or running) means the new plan was read. Said by the person's
  // own act, not by the drawer going away: the page takes it down for a
  // moment whenever the repository is being re-read, and the note that
  // upstream moved must still be there when it comes back.
  const close = () => {
    clearMoved(repoPath);
    onClose();
  };
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onStash = (v: boolean) => {
    setStash(v);
    // The blockers depend on this choice; re-plan locally, no second fetch.
    if (st.plan && !st.plan.refusal) void assess(repoPath, status, v, false);
  };

  const remedy = async (code: string, sub?: string) => {
    if (code === "turn-on-stash") {
      onStash(true);
      return;
    }
    // These take the drawer down: that is a close, and the "upstream moved
    // while you were reading" note is over with it.
    if (code === "open" || code === "untracked-in-the-way" || code === "choose-branch") clearMoved(repoPath);
    await onRemedy(code, sub);
    if (code === "record-pointers" || code === "reinit" || code === "stash-collides") setReplanWhenFresh(true);
  };

  const plan = st.plan;
  // A plan kept from earlier is not runnable while the repository itself says no.
  const canRun = !!plan && plan.can_run && !busy && !st.assessing && blocked === null;

  return (
    <div className="fixed inset-0 z-40 flex justify-end">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-[1px]" onClick={close} />
      <div
        role="dialog"
        aria-label="Update"
        className="relative h-full w-full lg:w-[40rem] bg-zinc-900 border-l border-zinc-800 shadow-2xl flex flex-col"
      >
        <div className="flex items-start justify-between gap-3 p-4 border-b border-zinc-800">
          <div className="min-w-0">
            <h2 className="text-sm font-medium text-zinc-100 flex items-center gap-2">
              <ArrowDownUp size={15} className="text-zinc-400 shrink-0" />
              Update: the latest from {upstream}, your work on top
            </h2>
            <p className="text-xs text-zinc-400 mt-1 leading-relaxed">
              Fetches, then replays your commits on top of what came in — rebasing the submodules that carry your work
              and fast-forwarding the rest. A backup branch is made first. Nothing is pushed.
            </p>
          </div>
          <button
            type="button"
            onClick={close}
            aria-label="Close"
            className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
          >
            <X size={18} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-3">
          {blocked && <p className="text-xs text-amber-400/90">{blocked}</p>}
          {!blocked && st.assessing && !plan && (
            <p className="flex items-center gap-2 text-sm text-zinc-300">
              <Loader2 size={14} className="animate-spin text-emerald-400 shrink-0" />
              Fetching from {upstream} — nothing in your repository changes until you press Update now.
            </p>
          )}
          {st.error && <p className="text-xs text-red-300 break-words">{st.error}</p>}
          {st.moved && (
            <p className="text-xs text-amber-300/90 break-words">
              {upstream} moved while you were reading — here is the new plan; check it and press Update now again.
            </p>
          )}
          {plan && (
            <SyncPlanView
              plan={plan}
              stash={stash}
              onStash={onStash}
              bundles={bundles}
              onBundles={setBundles}
              busy={busy || st.assessing}
              onRemedy={(code, sub) => void remedy(code, sub)}
            />
          )}
          {plan && !plan.refusal && (
            <details className="text-xs text-zinc-500">
              <summary className="cursor-pointer select-none hover:text-zinc-400">What git will run</summary>
              <ol className="mt-1 space-y-0.5 list-decimal list-inside font-mono">
                <li>git fetch {plan.superproject.remote || upstream.split("/")[0]}</li>
                <li>git branch gitswitch-before-update-&lt;time&gt; (a backup of where you are)</li>
                {plan.superproject.behind > 0 && (
                  <li>
                    git rebase {plan.superproject.will_stash ? "--autostash " : ""}
                    {rawUpstream} — your commits on top of what came in
                  </li>
                )}
                {plan.submodules.some((s) => s.action === "rebase") && (
                  <li>in each submodule that carries your commits: git rebase onto the commit the superproject records</li>
                )}
                {plan.submodules.some((s) => s.action === "fast-forward") && <li>in the others: git merge --ff-only to that commit</li>}
                <li>six checks: behind 0, your commits on top, your changes back, untracked untouched, submodules aligned, no conflicts</li>
              </ol>
            </details>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2 p-4 border-t border-zinc-800">
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || st.assessing || blocked !== null}
            title={blocked ?? "Fetches again, then shows the plan. Changes nothing."}
            onClick={() => void assess(repoPath, status, stash, true)}
          >
            {st.assessing ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            {st.assessing ? "Assessing…" : "Assess again"}
          </Button>
          {plan && !plan.refusal && !plan.nothing_to_do && (
            <Button
              size="sm"
              variant="primary"
              disabled={!canRun}
              title={plan.can_run ? "Runs exactly the plan above" : "Clear the blockers above first"}
              onClick={() => {
                clearMoved(repoPath);
                onRun(stash, bundles, plan.fingerprint);
              }}
              className="min-w-[8rem]"
            >
              <Play size={13} />
              Update now
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
