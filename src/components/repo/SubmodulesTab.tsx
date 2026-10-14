import { useCallback, useEffect, useState } from "react";
import { Box, Download, FolderOpen, GitGraph, GitBranch, Loader2, AlertTriangle, GitCommitHorizontal, ArrowDownUp } from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import * as api from "../../lib/api";
import type { SubmoduleInfo } from "../../lib/api";
import {
  StartBranchForm,
  canCheckOutRecorded,
  canPullInside,
  canRebaseLatest,
  rebaseLatestTitle,
  canRecord,
  canStartBranch,
  canUpdate,
  pullInsideTitle,
  recordTitle,
  recordedUnpushed,
  recordedUnpushedTitle,
  startBranchTitle,
  updateTitle,
  checkOutRecordedTitle,
} from "../changes/SubmodulesCard";

interface Props {
  /** The scoped repository — the repository, or a submodule inside it: this tab is about ITS submodules. */
  repoPath: string;
  /** The scope relative to the repository ("" for the repository itself), for the heading. */
  scope: string;
  visible: boolean;
  busy: boolean;
  /** Bumped by the page after any operation, so the table re-reads. */
  refreshKey: number;
  onWorkInside: (sub: string) => void;
  onOpenHistory: (sub: string) => void;
  onInitialise: (paths: string[]) => void;
  onRecordPointers: () => void;
  /** `git pull` inside one submodule, on its own branch — or, with `source`, a rebase onto that remote-tracking branch. */
  onPullInside: (sub: SubmoduleInfo, source?: string) => void;
  /** `git submodule update -- <path>`: check out the commit this repository records. */
  onUpdateOne: (path: string) => void;
  /** One commit in the repository recording where that submodule is checked out now. */
  onRecordOne: (path: string) => void;
  /** Checks out the commit the repository records, inside that submodule (local, its branches untouched). */
  onAlignOne: (path: string) => void;
  /** A branch at a detached submodule's checkout, named by the person. */
  onStartBranch: (path: string, name: string) => void;
  /** Opens the Update drawer for the repository: the latest below, your work on top, every submodule aligned. */
  onPreview: () => void;
}

function stateLabel(s: SubmoduleInfo): string {
  switch (s.state) {
    case "clean":
      return "as recorded";
    case "moved":
      return "moved";
    case "dirty":
      return "changes inside";
    case "moved-and-dirty":
      return "moved + changes inside";
    case "not-initialised":
      return "not downloaded";
    case "incomplete":
      return "download interrupted";
    case "unmapped":
      return "unfetchable";
  }
  return s.state;
}

function stateClass(s: SubmoduleInfo): string {
  switch (s.state) {
    case "moved":
    case "moved-and-dirty":
      return "bg-sky-500/15 text-sky-300";
    case "dirty":
    case "incomplete":
      return "bg-amber-500/15 text-amber-300";
    case "unmapped":
      return "bg-red-500/15 text-red-300";
    default:
      return "bg-zinc-700/50 text-zinc-400";
  }
}

/**
 * Every submodule of the repository in one table: where it is checked out,
 * what the repository records, how far its own branch is from its own
 * upstream, and what can be done about each one. For a superproject whose
 * upstream is a stream of pointer bumps, this is where the person's work
 * actually lives.
 */
export function SubmodulesTab({ repoPath, scope, visible, busy, refreshKey, onWorkInside, onOpenHistory, onInitialise, onRecordPointers, onPullInside, onUpdateOne, onRecordOne, onAlignOne, onStartBranch, onPreview }: Props) {
  // The row whose branch is being named — never one of another repository's.
  const [branchingFor, setBranchingFor] = useState<string | null>(null);
  useEffect(() => {
    setBranchingFor(null);
  }, [repoPath]);
  const [subs, setSubs] = useState<SubmoduleInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    if (!repoPath) {
      setSubs([]);
      return;
    }
    setLoading(true);
    api
      .changesSubmodules(repoPath)
      .then((s) => {
        setSubs(s);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [repoPath]);
  useEffect(load, [load, refreshKey]);

  // An interrupted download (a .git entry, nothing checked out) is not
  // populated: it is downloadable, like one never downloaded.
  const present = subs.filter((s) => s.initialised && s.state !== "incomplete");
  const missing = subs.filter((s) => (!s.initialised || s.state === "incomplete") && s.listed);
  const unfetchable = subs.filter((s) => !s.initialised && !s.listed);
  const moved = present.filter((s) => s.state === "moved" || s.state === "moved-and-dirty");

  return (
    <div className={`space-y-4 ${visible ? "" : "hidden"}`} role="tabpanel" aria-label="Submodules">
      {!repoPath ? (
        <Card>
          <p className="text-sm text-zinc-400">Pick a repository above.</p>
        </Card>
      ) : loading && subs.length === 0 ? (
        <p className="flex items-center gap-2 text-sm text-zinc-500">
          <Loader2 size={14} className="animate-spin text-emerald-400" />
          Reading submodules…
        </p>
      ) : subs.length === 0 && !error ? (
        <Card>
          <p className="text-sm text-zinc-400">{scope ? scope : "This repository"} has no submodules.</p>
        </Card>
      ) : (
        <>
          {error && <p className="text-sm text-red-400 break-words">{error}</p>}
          {scope && (
            <p className="text-xs text-zinc-500">
              Submodules of <span className="font-mono text-zinc-300">{scope}</span> — downloaded, recorded and pulled from here; Update works on the repository as a whole.
            </p>
          )}
          <div className="flex items-center gap-2 flex-wrap text-xs text-zinc-500">
            <Box size={14} className="text-zinc-400" />
            <span>
              {present.length} populated
              {moved.length > 0 && <> · <span className="text-sky-300">{moved.length} moved</span></>}
              {missing.length > 0 && <> · {missing.length} not downloaded yet</>}
              {unfetchable.length > 0 && <> · <span className="text-red-300">{unfetchable.length} unfetchable</span></>}
            </span>
            <span className="ml-auto flex items-center gap-2">
              {moved.length > 0 && (
                <Button size="sm" variant="primary" disabled={busy} onClick={onPreview} title="Fetches, then shows what an update would do to the repository and every submodule — nothing changes until you press Update now">
                  <ArrowDownUp size={13} />
                  Preview update
                </Button>
              )}
              {moved.length > 0 && (
                <Button size="sm" variant="secondary" disabled={busy} onClick={onRecordPointers} title="Commit the superproject's record of where each submodule is checked out now">
                  <GitCommitHorizontal size={13} />
                  Record submodule pointers
                </Button>
              )}
              {missing.length > 0 && (
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={busy}
                  onClick={() => onInitialise(missing.map((s) => s.path))}
                  title={`git submodule update --init for ${missing.map((s) => s.path).join(", ")} — populated ones are left alone`}
                >
                  <Download size={13} />
                  Download missing ({missing.length})
                </Button>
              )}
            </span>
          </div>

          <Card className="p-0 overflow-hidden">
            <table className="w-full text-sm">
              <thead className="text-xs text-zinc-500 border-b border-zinc-800">
                <tr>
                  <th className="text-left font-medium px-3 py-2">Submodule</th>
                  <th className="text-left font-medium px-3 py-2 hidden md:table-cell">Checked out</th>
                  <th className="text-left font-medium px-3 py-2 hidden lg:table-cell">Its own branch</th>
                  <th className="text-right font-medium px-3 py-2">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-800/70">
                {present.map((s) => (
                  <tr key={s.path} className="align-top">
                    <td className="px-3 py-2 min-w-0">
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="font-mono text-xs text-zinc-200 break-all">{s.path}</span>
                        <span className={`text-[10px] px-1.5 py-0.5 rounded shrink-0 ${stateClass(s)}`}>{stateLabel(s)}</span>
                        {recordedUnpushed(s) && (
                          <span className="text-[10px] px-1.5 py-0.5 rounded shrink-0 bg-amber-500/15 text-amber-300" title={recordedUnpushedTitle(s)}>
                            recorded commit not pushed
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-zinc-500 mt-0.5 leading-relaxed">{s.summary}</p>
                      <p className="lg:hidden text-xs font-mono text-zinc-500 mt-0.5">
                        {s.own_branch ?? "detached"}
                        {s.own_upstream ? ` → ${s.own_upstream}` : ""}
                        {s.own_ahead > 0 ? ` ↑${s.own_ahead}` : ""}
                        {s.own_behind > 0 ? ` ↓${s.own_behind}` : ""}
                      </p>
                    </td>
                    <td className="px-3 py-2 hidden md:table-cell text-xs font-mono text-zinc-400 whitespace-nowrap">
                      {s.actual_short ?? "—"}
                      {s.actual_short && s.actual_short !== s.recorded_short && (
                        <span className="block text-zinc-600">records {s.recorded_short}</span>
                      )}
                    </td>
                    <td className="px-3 py-2 hidden lg:table-cell text-xs text-zinc-400 whitespace-nowrap">
                      {s.own_branch ? (
                        <>
                          <span className="font-mono text-zinc-300">{s.own_branch}</span>
                          {s.own_upstream && <span className="text-zinc-600"> → {s.own_upstream}</span>}
                          {s.own_ahead > 0 && <span className="text-emerald-400"> ↑{s.own_ahead}</span>}
                          {s.own_behind > 0 && <span className="text-amber-400"> ↓{s.own_behind}</span>}
                        </>
                      ) : (
                        <span className="text-amber-300">detached</span>
                      )}
                      {(s.dirty_tracked > 0 || s.dirty_untracked > 0) && (
                        <span className="block text-amber-300/90">
                          {s.dirty_tracked > 0 ? `${s.dirty_tracked} changed` : ""}
                          {s.dirty_tracked > 0 && s.dirty_untracked > 0 ? " · " : ""}
                          {s.dirty_untracked > 0 ? `${s.dirty_untracked}${s.dirty_untracked_capped ? "+" : ""} new` : ""}
                        </span>
                      )}
                    </td>
                    <td className="px-3 py-2 text-right">
                      <div className="flex flex-wrap justify-end items-center gap-0.5">
                        {canPullInside(s) && (
                          <Button size="sm" variant={s.own_behind > 0 ? "secondary" : "ghost"} disabled={busy} onClick={() => onPullInside(s)} title={pullInsideTitle(s)} className="px-2">
                            <Download size={13} />
                            Pull inside{s.own_behind > 0 ? ` (${s.own_behind})` : ""}
                          </Button>
                        )}
                        {canRebaseLatest(s) && (
                          <Button size="sm" variant="ghost" disabled={busy} onClick={() => onPullInside(s, s.latest_ref ?? undefined)} title={rebaseLatestTitle(s)} className="px-2">
                            <Download size={13} />
                            Rebase onto {s.latest_ref}
                          </Button>
                        )}
                        {canUpdate(s) && (
                          <Button size="sm" variant="secondary" disabled={busy} onClick={() => onUpdateOne(s.path)} title={updateTitle(s)} className="px-2">
                            <GitCommitHorizontal size={13} />
                            Update to {s.recorded_short}
                          </Button>
                        )}
                        {canRecord(s) && (
                          <Button size="sm" variant="secondary" disabled={busy} onClick={() => onRecordOne(s.path)} title={recordTitle(s)} className="px-2">
                            <GitCommitHorizontal size={13} />
                            Record this pointer
                          </Button>
                        )}
                        {canCheckOutRecorded(s) && (
                          <Button size="sm" variant="secondary" disabled={busy} onClick={() => onAlignOne(s.path)} title={checkOutRecordedTitle(s)} className="px-2">
                            <GitCommitHorizontal size={13} />
                            Check out {s.recorded_short}
                          </Button>
                        )}
                        {canStartBranch(s) && branchingFor !== s.path && (
                          <Button size="sm" variant={(s.own_unpushed ?? 0) > 0 ? "secondary" : "ghost"} disabled={busy} onClick={() => setBranchingFor(s.path)} title={startBranchTitle(s)} className="px-2">
                            <GitBranch size={13} />
                            Start a branch here
                          </Button>
                        )}
                        {canStartBranch(s) && branchingFor === s.path && (
                          <StartBranchForm
                            path={s.path}
                            busy={busy}
                            onSubmit={(name) => {
                              setBranchingFor(null);
                              onStartBranch(s.path, name);
                            }}
                            onCancel={() => setBranchingFor(null)}
                          />
                        )}
                        <Button size="sm" variant="ghost" onClick={() => onWorkInside(s.path)} title="Work in this submodule as its own repository" className="px-2">
                          <FolderOpen size={13} />
                          Work inside
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => onOpenHistory(s.path)} title="Browse this submodule's own history" className="px-2">
                          <GitGraph size={13} />
                          History
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
                {[...missing, ...unfetchable].map((s) => (
                  <tr key={s.path} className="align-top text-zinc-500">
                    <td className="px-3 py-2 min-w-0" colSpan={3}>
                      <div className="flex items-center gap-2 flex-wrap">
                        <AlertTriangle size={12} className="shrink-0 text-zinc-600" />
                        <span className="font-mono text-xs break-all">{s.path}</span>
                        <span className={`text-[10px] px-1.5 py-0.5 rounded shrink-0 ${stateClass(s)}`}>{stateLabel(s)}</span>
                      </div>
                      <p className="text-xs mt-0.5 leading-relaxed">{s.summary}</p>
                    </td>
                    <td className="px-3 py-2 text-right whitespace-nowrap">
                      {s.listed && (
                        <Button size="sm" variant="secondary" disabled={busy} onClick={() => onInitialise([s.path])} className="px-2" title="git submodule update --init for this one submodule: downloads it at the commit this repository records">
                          <Download size={13} />
                          Download
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>
          {unfetchable.length > 0 && (
            <p className="text-xs text-zinc-500">
              {unfetchable.length === 1 ? "One submodule has" : `${unfetchable.length} submodules have`} no entry in .gitmodules, so git has no address to fetch from — the same as any clone.
            </p>
          )}
        </>
      )}
    </div>
  );
}
