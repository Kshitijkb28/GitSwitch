import { useCallback, useEffect, useState } from "react";
import { Layers, Loader2 } from "lucide-react";
import { Button } from "../Button";
import { Modal } from "../Modal";
import { cleanError } from "../github/shared";
import * as api from "../../lib/api";
import type { OpResult, RepoState, RepoStatus, StatesList } from "../../lib/api";

type Props = {
  repoPath: string;
  status: RepoStatus;
  busy: boolean;
  /** Bumped by the page after any operation, so the list re-reads. */
  refreshKey: number;
  /** The page's `apply`: runs the operation, adopts the status, shows the result. */
  run: (key: string, fn: () => Promise<OpResult>) => Promise<OpResult | undefined>;
};

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "just now", "12 min ago", "3 h ago", or the day. */
function when(saved: number): string {
  const s = Math.max(0, Math.round(Date.now() / 1000 - saved));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return new Date(saved * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

function slug(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 30);
}

/** What the files come back as, in a few words. */
function filesLine(s: RepoState): string {
  const parts = [plural(s.changed, "changed file", "changed files")];
  if (s.staged > 0) parts.push(`${s.staged} staged`);
  if (s.new_files > 0) parts.push(`${s.new_files} new`);
  return parts.length > 1 ? `${parts[0]} (${parts.slice(1).join(", ")})` : parts[0];
}

/** Where HEAD goes, in one sentence. */
function headLine(s: RepoState): string {
  const at = `${s.head_short}${s.head_subject ? ` “${s.head_subject}”` : ""}`;
  if (!s.branch) return `HEAD goes to ${at} (detached).`;
  if (s.branch_gone) return `${s.branch} was deleted since; it is made again at ${at}.`;
  if (s.commits_since > 0) {
    return `${s.branch} moves back to ${at}. The ${plural(s.commits_since, "commit", "commits")} made since leave it and are kept on a backup branch.`;
  }
  return `${s.branch} is at ${at}.`;
}

/**
 * Repository states: four slots, each holding where the repository was — the
 * commit, the branch, what was staged and every changed or new file, with the
 * repositories inside it — and a way back to any of them. Going back saves
 * what is there first as "Before going back", so it is never a loss.
 *
 * Every action names the state by its commit: the list may be out of date
 * (another window, a terminal), and the backend refuses (`no-state`) rather
 * than act on something else.
 */
export function StatesCard({ repoPath, status, busy, refreshKey, run }: Props) {
  const [list, setList] = useState<StatesList | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const [saveFor, setSaveFor] = useState<{ slot: string; replace: RepoState | null } | null>(null);
  const [name, setName] = useState("");
  const [backTo, setBackTo] = useState<RepoState | null>(null);
  const [newBranch, setNewBranch] = useState("");
  const [deleteFor, setDeleteFor] = useState<RepoState | null>(null);
  /** The "keep up to" field as typed, and why the last number was refused. */
  const [maxDraft, setMaxDraft] = useState("4");
  const [maxError, setMaxError] = useState<string | null>(null);

  const load = useCallback(() => {
    if (!repoPath) return;
    let cancelled = false;
    setLoading(true);
    api
      .statesList(repoPath)
      .then((l) => {
        if (cancelled) return;
        setList(l);
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath]);

  // Re-read after any in-app operation, and when HEAD moves (a commit, a terminal).
  useEffect(load, [load, refreshKey, status.head_oid, status.branch]);

  const act = async (fn: () => Promise<OpResult>) => {
    const r = await run("states", fn);
    if (r?.refusal?.code === "no-state") setStale(true);
    else if (r?.ok) setStale(false);
    return r;
  };

  const max = list?.max ?? 4;
  const limit = list?.limit ?? 99;
  const states = list?.states ?? [];
  const nextFree = list ? list.next_free : null;
  const before = list?.before ?? null;

  // The field follows the repository's number whenever the list is read.
  useEffect(() => {
    if (list) setMaxDraft(String(list.max));
  }, [list]);

  /** A new "keep up to" number: the backend refuses one below a saved state. */
  const applyMax = async () => {
    const n = Number.parseInt(maxDraft, 10);
    if (!Number.isFinite(n) || n === max) {
      setMaxDraft(String(max));
      return;
    }
    try {
      setList(await api.statesSetMax(repoPath, n));
      setMaxError(null);
    } catch (e) {
      setMaxError(cleanError(e));
      setMaxDraft(String(max));
    }
  };

  const openSave = (slot: string, replace: RepoState | null) => {
    setName(replace?.name ?? "");
    setSaveFor({ slot, replace });
  };
  const doSave = async () => {
    const s = saveFor;
    setSaveFor(null);
    if (!s) return;
    await act(() => api.statesSave(repoPath, s.slot, name.trim() || null, s.replace?.oid ?? null));
  };
  const openBack = (s: RepoState) => {
    setNewBranch(`${s.branch ?? "state"}-${slug(s.name) || `state-${s.slot}`}`);
    setBackTo(s);
  };
  const goBack = async (onNewBranch: string | null, allowPushed: boolean) => {
    const s = backTo;
    setBackTo(null);
    if (!s) return;
    await act(() => api.statesRestore(repoPath, s.slot, s.oid, onNewBranch, allowPushed));
  };
  const doDelete = async () => {
    const s = deleteFor;
    setDeleteFor(null);
    if (!s) return;
    await act(() => api.statesDelete(repoPath, s.slot, s.oid));
  };

  const changesNow = status.entries.length;
  const subsPushed = (backTo?.subs ?? []).filter((s) => s.pushed_since > 0);

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40" data-states-card>
      <div className="flex items-center gap-2 px-3 py-2 border-b border-zinc-700/50">
        <Layers size={14} className="text-zinc-400 shrink-0" />
        <h3 className="text-sm font-medium text-zinc-200 truncate min-w-0">
          States{" "}
          <span className="text-xs font-normal text-zinc-500">
            {states.length} of {max}
          </span>
        </h3>
        {loading && <Loader2 size={12} className="animate-spin text-zinc-500 shrink-0" />}
      </div>
      <p className="px-3 pt-2 text-[11px] text-zinc-500 break-words">
        Save where you are — the commit, the branch and every changed file, the repositories inside too — and come back
        to it.
      </p>
      <label className="px-3 pt-1.5 flex items-center gap-1.5 text-[11px] text-zinc-500" data-state-max>
        Keep up to
        <input
          type="number"
          min={1}
          max={limit}
          value={maxDraft}
          onChange={(e) => setMaxDraft(e.target.value)}
          onBlur={() => void applyMax()}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
          disabled={busy || !list}
          aria-label="How many states to keep"
          className="h-6 w-14 rounded bg-zinc-900 border border-zinc-700 px-1.5 text-xs text-zinc-200 focus:outline-none focus:border-zinc-500"
        />
        states in this repository
      </label>
      {maxError && (
        <p className="px-3 pt-1 text-xs text-red-400 break-words" data-state-max-error>
          {maxError}
        </p>
      )}

      {error && <p className="px-3 py-2 text-xs text-red-400 break-words">{error}</p>}
      {stale && (
        <p className="px-3 py-2 text-xs text-amber-300/90 break-words">
          The list was out of date — a state changed outside this window. It has been read again; nothing was changed.
        </p>
      )}

      <ul className="divide-y divide-zinc-800/70 max-h-[28rem] overflow-y-auto">
        {states.map((s) => {
          const slot = s.slot;
          return (
            <li key={slot} className="px-3 py-2 space-y-1" data-state-slot={slot}>
              <div className="flex items-center gap-2 min-w-0">
                <span className="font-mono text-[11px] text-zinc-500 shrink-0">{slot}</span>
                <span className="text-xs font-medium text-zinc-100 truncate min-w-0" title={s.name}>
                  {s.name}
                </span>
                <span className="text-[11px] text-zinc-500 ml-auto shrink-0">{when(s.saved)}</span>
              </div>
              <p className="text-[11px] text-zinc-400 truncate" title={`${s.branch ?? "detached HEAD"} · ${s.head_short} ${s.head_subject}`}>
                <span className="font-mono text-zinc-300">{s.branch ?? "detached"}</span> ·{" "}
                <span className="font-mono">{s.head_short}</span> {s.head_subject}
              </p>
              <p className="text-[11px] text-zinc-500 break-words">
                {filesLine(s)}
                {s.subs.length > 0 && ` · ${plural(s.subs.length, "repository", "repositories")} inside`}
                {s.branch && s.commits_since > 0 && ` · ${s.branch} has ${plural(s.commits_since, "commit", "commits")} since`}
                {s.branch && s.branch_gone && ` · ${s.branch} was deleted`}
              </p>
              <div className="flex items-center gap-0.5 flex-wrap">
                <Button size="sm" variant="ghost" className="px-1.5" disabled={busy} data-state-back={slot} onClick={() => openBack(s)}>
                  Go back
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="px-1.5"
                  disabled={busy}
                  data-state-replace={slot}
                  title="Save where you are now into this slot instead"
                  onClick={() => openSave(slot, s)}
                >
                  Save here again
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="px-1.5 hover:text-red-400"
                  disabled={busy}
                  data-state-delete={slot}
                  onClick={() => setDeleteFor(s)}
                >
                  Delete…
                </Button>
              </div>
            </li>
          );
        })}
        {list && nextFree && (
          <li className="px-3 py-2 flex items-center gap-2" data-state-new>
            <span className="text-xs text-zinc-500">{states.length === 0 ? "No state saved yet." : `${max - states.length} more can be saved.`}</span>
            <Button
              size="sm"
              variant="ghost"
              className="px-1.5 ml-auto"
              disabled={busy}
              data-state-save={nextFree}
              title="Save the commit, the branch and every changed file as they are now"
              onClick={() => openSave(nextFree, null)}
            >
              Save a new state
            </Button>
          </li>
        )}
        {list && !nextFree && (
          <li className="px-3 py-2 text-xs text-amber-300/90 break-words" data-state-full>
            All {max} are in use. Save over one (Save here again), delete one, or keep more states above.
          </li>
        )}
      </ul>

      {before && (
        <div className="px-3 py-2 border-t border-zinc-700/50 space-y-1 bg-zinc-900/30" data-state-before>
          <div className="flex items-center gap-2 min-w-0">
            <span className="text-xs font-medium text-zinc-200 truncate min-w-0">
              Before going back{before.before_of ? ` to “${before.before_of}”` : ""}
            </span>
            <span className="text-[11px] text-zinc-500 ml-auto shrink-0">{when(before.saved)}</span>
          </div>
          <p className="text-[11px] text-zinc-500 break-words">
            Everything you had then: <span className="font-mono text-zinc-400">{before.branch ?? "detached"}</span> at{" "}
            <span className="font-mono">{before.head_short}</span>, {filesLine(before)}
            {before.subs.length > 0 && `, ${plural(before.subs.length, "repository", "repositories")} inside`}.
          </p>
          <div className="flex items-center gap-0.5">
            <Button size="sm" variant="ghost" className="px-1.5" disabled={busy} data-state-back="before" onClick={() => openBack(before)}>
              Go back to it
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="px-1.5 hover:text-red-400"
              disabled={busy}
              data-state-delete="before"
              onClick={() => setDeleteFor(before)}
            >
              Forget it…
            </Button>
          </div>
        </div>
      )}

      <Modal
        open={saveFor !== null}
        onClose={() => setSaveFor(null)}
        title={saveFor?.replace ? `Replace “${saveFor.replace.name}”?` : `Save state ${saveFor?.slot ?? ""}`}
      >
        {saveFor && (
          <div className="space-y-3" data-state-modal="save">
            <p className="text-sm text-zinc-300 break-words">
              Saves where you are now: <span className="font-mono text-zinc-100">{status.branch ?? "detached HEAD"}</span>
              {status.head_oid && (
                <>
                  {" "}
                  at <span className="font-mono">{status.head_oid.slice(0, 7)}</span>
                </>
              )}{" "}
              with {plural(changesNow, "changed file", "changed files")} — staged or not, new files too — and the
              repositories inside it.
            </p>
            {saveFor.replace && (
              <p className="text-xs text-amber-300/90 break-words">
                “{saveFor.replace.name}” ({saveFor.replace.branch ?? "detached"} at {saveFor.replace.head_short}) leaves this slot.
              </p>
            )}
            <input
              autoFocus
              value={name}
              maxLength={60}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void doSave();
                if (e.key === "Escape") setSaveFor(null);
              }}
              placeholder={`State ${saveFor.slot}`}
              aria-label="State name"
              className="h-8 w-full rounded bg-zinc-900 border border-zinc-700 px-2 text-sm text-zinc-200 focus:outline-none focus:border-zinc-500"
            />
            <p className="text-xs text-zinc-500">Ignored files aren't part of a state. Nothing is pushed anywhere.</p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" size="sm" onClick={() => setSaveFor(null)}>
                Cancel
              </Button>
              <Button variant="primary" size="sm" data-state-confirm onClick={() => void doSave()}>
                {saveFor.replace ? "Replace" : "Save"}
              </Button>
            </div>
          </div>
        )}
      </Modal>

      <Modal open={backTo !== null} onClose={() => setBackTo(null)} title={backTo ? `Go back to “${backTo.name}”?` : ""} size="lg">
        {backTo && (
          <div className="space-y-3" data-state-modal="back">
            <ul className="list-disc pl-5 space-y-1 text-sm text-zinc-300 break-words">
              <li>{headLine(backTo)}</li>
              <li>
                Your files become exactly as saved: {filesLine(backTo)}. Files made since that aren't ignored are removed.
              </li>
              {backTo.subs.length > 0 && <li>The repositories inside go back too: {backTo.subs.map((s) => s.path).join(", ")}.</li>}
              <li>Everything you have now is saved first as “Before going back”, so you can return.</li>
            </ul>
            {backTo.branch && backTo.pushed_since > 0 ? (
              <div className="rounded-md border border-amber-500/30 bg-amber-500/5 p-3 space-y-2" data-state-pushed>
                <p className="text-xs text-amber-200 break-words">
                  {plural(backTo.pushed_since, "commit", "commits")} made since {backTo.pushed_since === 1 ? "is" : "are"} already
                  pushed to {backTo.upstream ?? "a remote"}. Moving {backTo.branch} back makes it disagree with the remote: your
                  next push would be refused unless forced (which deletes {backTo.pushed_since === 1 ? "it" : "them"} there), and
                  Update would bring {backTo.pushed_since === 1 ? "it" : "them"} back. On a new branch, {backTo.branch} stays as
                  it is.
                </p>
                <label className="flex items-center gap-2 text-xs text-zinc-400">
                  New branch
                  <input
                    value={newBranch}
                    onChange={(e) => setNewBranch(e.target.value)}
                    aria-label="New branch name"
                    className="h-7 flex-1 min-w-0 rounded bg-zinc-900 border border-zinc-700 px-2 text-xs text-zinc-200 font-mono focus:outline-none focus:border-zinc-500"
                  />
                </label>
                <div className="flex justify-end gap-2 flex-wrap">
                  <Button variant="secondary" size="sm" onClick={() => setBackTo(null)}>
                    Cancel
                  </Button>
                  <Button variant="danger" size="sm" data-state-anyway onClick={() => void goBack(null, true)}>
                    Move {backTo.branch} back anyway
                  </Button>
                  <Button
                    variant="primary"
                    size="sm"
                    data-state-new-branch
                    disabled={!newBranch.trim()}
                    onClick={() => void goBack(newBranch.trim(), false)}
                  >
                    Go back on a new branch
                  </Button>
                </div>
              </div>
            ) : (
              <div className="flex justify-end gap-2">
                <Button variant="secondary" size="sm" onClick={() => setBackTo(null)}>
                  Cancel
                </Button>
                <Button variant="primary" size="sm" data-state-confirm onClick={() => void goBack(null, false)}>
                  Go back
                </Button>
              </div>
            )}
            {subsPushed.length > 0 && (
              <p className="text-xs text-zinc-500 break-words" data-state-subs-pushed>
                {subsPushed.map((s) => s.path).join(", ")} {subsPushed.length === 1 ? "has" : "have"} pushed commits since: unless
                you move back anyway, {subsPushed.length === 1 ? "it is" : "they are"} put back at the saved commit and{" "}
                {subsPushed.length === 1 ? "its branch keeps them" : "their branches keep them"}.
              </p>
            )}
          </div>
        )}
      </Modal>

      <Modal open={deleteFor !== null} onClose={() => setDeleteFor(null)} title={deleteFor ? `Delete “${deleteFor.name}”?` : ""}>
        {deleteFor && (
          <div className="space-y-3" data-state-modal="delete">
            <p className="text-sm text-zinc-300 break-words">
              It leaves the list: {deleteFor.branch ?? "detached"} at {deleteFor.head_short}, {filesLine(deleteFor)}.
            </p>
            <p className="text-xs text-zinc-500">Its commit id is shown afterwards, so it can still be recovered for a while.</p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" size="sm" onClick={() => setDeleteFor(null)}>
                Keep it
              </Button>
              <Button variant="danger" size="sm" data-state-confirm onClick={() => void doDelete()}>
                Delete
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
