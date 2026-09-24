import { useCallback, useEffect, useState } from "react";
import { Archive, Loader2 } from "lucide-react";
import { Badge } from "../Badge";
import { Button } from "../Button";
import { Modal } from "../Modal";
import * as api from "../../lib/api";
import type { OpResult, RepoStatus, StashDetail, StashEntry } from "../../lib/api";

type Props = {
  repoPath: string;
  status: RepoStatus;
  busy: boolean;
  /** Bumped by the page after any operation, so the list re-reads. */
  refreshKey: number;
  /** The page's `apply`: runs the operation, adopts the status, shows the result. */
  run: (key: string, fn: () => Promise<OpResult>) => Promise<OpResult | undefined>;
};

/** "YYYY-MM-DD" from whatever date string git gave. */
function day(d: string): string {
  if (/^\d{4}-\d{2}-\d{2}/.test(d)) return d.slice(0, 10);
  const t = new Date(d);
  return isNaN(t.getTime()) ? d : t.toISOString().slice(0, 10);
}

type Detail = { state: "loading" } | { state: "error"; error: string } | { state: "ok"; detail: StashDetail };

/**
 * The stash list, with the two ways back (Apply keeps the entry, Pop removes
 * it) and a way to pull a single file out. Dropping asks first, because the
 * only undo is a commit id nobody wrote down.
 */
export function StashesCard({ repoPath, status, busy, refreshKey, run }: Props) {
  const [list, setList] = useState<StashEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [details, setDetails] = useState<Record<number, Detail>>({});
  const [openFiles, setOpenFiles] = useState<Record<number, boolean>>({});
  const [dropFor, setDropFor] = useState<StashEntry | null>(null);

  const load = useCallback(() => {
    if (!repoPath) return;
    let cancelled = false;
    setLoading(true);
    api
      .changesStashList(repoPath)
      .then((s) => {
        if (cancelled) return;
        setList(s);
        setError(null);
        // Indices shift when an entry is popped or dropped: what was loaded
        // for stash@{1} is now stash@{0}'s neighbour, so re-read on demand.
        setDetails({});
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

  useEffect(load, [load, refreshKey]);

  const count = list.length > 0 ? list.length : status.stash_count;
  if (count === 0 && list.length === 0 && !error) return null;

  const toggleFiles = (e: StashEntry) => {
    const next = !openFiles[e.index];
    setOpenFiles((o) => ({ ...o, [e.index]: next }));
    if (next && !details[e.index]) {
      setDetails((d) => ({ ...d, [e.index]: { state: "loading" } }));
      api
        .changesStashShow(repoPath, e.index)
        .then((detail) => setDetails((d) => ({ ...d, [e.index]: { state: "ok", detail } })))
        .catch((err) => setDetails((d) => ({ ...d, [e.index]: { state: "error", error: String(err) } })));
    }
  };

  const drop = async () => {
    const e = dropFor;
    setDropFor(null);
    if (!e) return;
    await run("stash", () => api.changesStashDrop(repoPath, e.index));
  };

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-zinc-700/50">
        <Archive size={14} className="text-zinc-400 shrink-0" />
        <h3 className="text-sm font-medium text-zinc-200 truncate min-w-0">
          Stashes <span className="text-xs font-normal text-zinc-500">{count}</span>
        </h3>
        {loading && <Loader2 size={12} className="animate-spin text-zinc-500 shrink-0" />}
      </div>

      {error && <p className="px-3 py-2 text-xs text-red-400 break-words">{error}</p>}

      <ul className="max-h-96 overflow-y-auto divide-y divide-zinc-800/70">
        {list.map((e) => {
          const files = e.tracked_files + e.untracked_files;
          const d = details[e.index];
          return (
            <li key={e.oid} className="px-3 py-2 space-y-1">
              <div className="flex items-center gap-2 min-w-0">
                <span className="font-mono text-xs text-zinc-400 shrink-0">{e.ref}</span>
                <span className="text-xs text-zinc-200 truncate min-w-0" title={e.message}>
                  {e.message}
                </span>
                {e.is_autostash && <Badge variant="warning">autostash</Badge>}
              </div>
              <div className="flex items-center gap-x-2 gap-y-1 flex-wrap text-[11px] text-zinc-500">
                {e.branch && (
                  <span>
                    on <span className="font-mono text-zinc-400">{e.branch}</span>
                  </span>
                )}
                <span>{day(e.date)}</span>
                <button
                  onClick={() => toggleFiles(e)}
                  className="text-zinc-500 hover:text-zinc-300 cursor-pointer"
                  title="List the files this stash holds"
                >
                  {files} file{files === 1 ? "" : "s"}
                </button>
                <div className="flex items-center gap-0.5 ml-auto">
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    className="px-1.5"
                    title="Apply this stash and keep it in the list"
                    onClick={() => run("stash", () => api.changesStashApply(repoPath, e.index, false, false))}
                  >
                    Apply
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    className="px-1.5"
                    title="Apply this stash and remove it"
                    onClick={() => run("stash", () => api.changesStashApply(repoPath, e.index, true, false))}
                  >
                    Pop
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    className="px-1.5 hover:text-red-400"
                    title="Drop this stash"
                    onClick={() => setDropFor(e)}
                  >
                    Drop…
                  </Button>
                </div>
              </div>

              {openFiles[e.index] && (
                <div className="pt-1">
                  {(!d || d.state === "loading") && (
                    <p className="flex items-center gap-1.5 text-xs text-zinc-500">
                      <Loader2 size={12} className="animate-spin" />
                      Reading the stash…
                    </p>
                  )}
                  {d?.state === "error" && <p className="text-xs text-red-400 break-words">{d.error}</p>}
                  {d?.state === "ok" && (
                    <ul className="space-y-0.5">
                      {d.detail.files.map((f) => (
                        <li key={f.path} className="flex items-center gap-2 min-w-0">
                          <span className="font-mono text-[11px] text-zinc-500 w-14 shrink-0">
                            {f.status === "untracked" ? "new" : f.status}
                          </span>
                          <span className="font-mono text-xs text-zinc-300 truncate min-w-0" title={f.path}>
                            {f.path}
                          </span>
                          <Button
                            size="sm"
                            variant="ghost"
                            disabled={busy}
                            className="px-1.5 ml-auto"
                            title="Restore only this file from the stash"
                            onClick={() =>
                              run("stash", () => api.changesStashRestoreFile(repoPath, e.index, f.path))
                            }
                          >
                            Restore file
                          </Button>
                        </li>
                      ))}
                      {d.detail.files.length === 0 && (
                        <li className="text-xs text-zinc-500">No files recorded.</li>
                      )}
                    </ul>
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>

      <Modal open={dropFor !== null} onClose={() => setDropFor(null)} title="Drop this stash?">
        {dropFor && (
          <div className="space-y-3">
            <p className="text-sm text-zinc-300 break-words">
              <span className="font-mono text-zinc-100">{dropFor.ref}</span> — {dropFor.message} (
              {dropFor.tracked_files + dropFor.untracked_files} file
              {dropFor.tracked_files + dropFor.untracked_files === 1 ? "" : "s"}). It leaves the list.
            </p>
            <p className="text-xs text-zinc-500">
              Git keeps the commit for a while; its id is shown afterwards so it can be recovered.
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" size="sm" onClick={() => setDropFor(null)}>
                Keep it
              </Button>
              <Button variant="danger" size="sm" onClick={drop}>
                Drop
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
