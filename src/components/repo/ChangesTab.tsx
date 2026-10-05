import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, AlertTriangle, CircleSlash, Play } from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import { useToast } from "../Toast";
import { TreeLists, type DiscardVariant } from "../changes/TreeLists";
import { CommitBox } from "../changes/CommitBox";
import { DiffPanel } from "../changes/DiffPanel";
import { TidyUpStrip } from "./TidyUpStrip";
import type { RepoActions } from "./StatusStrip";
import { BranchesModal, type RunOptions } from "../changes/BranchesModal";
import { PushAccessCard } from "../changes/PushAccessCard";
import { SubmodulesCard } from "../changes/SubmodulesCard";
import { SubmoduleSection, needsSection, sectionId } from "../changes/SubmoduleSection";
import { LfsCard } from "../changes/LfsCard";
import { LfsFilesModal, formatBytes } from "../changes/LfsFilesModal";
import { StashesCard } from "../changes/StashesCard";
import { StashModal } from "../changes/StashModal";
import { SyncCard, SyncPausedCard } from "../changes/SyncCard";
import { ResultCard } from "../changes/ResultCard";
import { DiscardConfirmModal } from "../changes/DiscardConfirmModal";
import { ResetConfirmModal } from "../changes/ResetConfirmModal";
import { DiscardAllModal } from "../changes/DiscardAllModal";
import { usePersistedState } from "../../lib/persist";
import { useRefreshOnFocus } from "../../lib/focus";
import { elapsedLabel, runJob, useGitJob, clearJob, type GitJobKind } from "../../lib/gitJobs";
import * as api from "../../lib/api";
import type { ChangeEntry, OpResult, PullMode, RepoStatus, SubmoduleInfo, SubmoduleStatus } from "../../lib/api";

type DiscardRequest = { paths: string[]; variant: DiscardVariant; target?: string };

interface Props {
  /** The repository — or the submodule in scope — this tab works on. */
  repoPath: string;
  visible: boolean;
  pullMode: PullMode;
  onPullMode: (m: PullMode) => void;
  pullLfs: boolean;
  onPullLfs: (v: boolean) => void;
  pullAutostash: boolean;
  onPullAutostash: (v: boolean) => void;
  /** Open another folder (a submodule as its own repository, or any path) on this page. */
  onOpenRepo: (path: string) => void;
  /** The status this tab read, for the strip above the tabs. */
  onStatus: (s: RepoStatus | null, loading: boolean) => void;
  /** The operations the strip's buttons run. Re-registered on every render. */
  onActions: (a: RepoActions) => void;
  onOpenHistory: () => void;
}

/**
 * The working tree: what used to be the whole Changes page, minus its picker
 * and branch line, which now sit above the tabs. It owns the status, the job
 * slot's results and every operation on this repository; the strip above only
 * calls into it.
 */
export function ChangesTab({
  repoPath,
  visible,
  pullMode,
  onPullMode,
  pullLfs,
  onPullLfs,
  pullAutostash,
  onPullAutostash,
  onOpenRepo,
  onStatus,
  onActions,
  onOpenHistory,
}: Props) {
  const toast = useToast();
  const [drafts, setDrafts] = usePersistedState<Record<string, string>>("changes.drafts", {});

  // Deliberately not persisted: amending must be a fresh decision every time,
  // and status/results must always be re-read rather than restored.
  const [amend, setAmend] = useState(false);
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [subStatuses, setSubStatuses] = useState<SubmoduleStatus[]>([]);
  const [openSubs, setOpenSubs] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<OpResult | null>(null);
  const [diffFor, setDiffFor] = useState<ChangeEntry | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState<DiscardRequest | null>(null);
  const [branchesFor, setBranchesFor] = useState<string | null>(null);
  const [lfsBrowser, setLfsBrowser] = useState(false);
  const [lfsProgress, setLfsProgress] = useState<api.LfsProgress | null>(null);
  const [stashOpen, setStashOpen] = useState(false);
  const [resetOpen, setResetOpen] = useState(false);
  const [discardAllOpen, setDiscardAllOpen] = useState(false);
  const [tick, setTick] = useState(0);
  const [subRefresh, setSubRefresh] = useState(0);

  const job = useGitJob(repoPath);
  const seq = useRef(0);
  const subSeq = useRef(0);

  /** Each submodule's own status, for its section. Only asked when there are any. */
  const loadSubStatuses = useCallback((path: string, hasSubmodules: boolean) => {
    const mine = ++subSeq.current;
    if (!path || !hasSubmodules) {
      setSubStatuses([]);
      return;
    }
    api
      .changesSubmoduleStatuses(path)
      .then((list) => {
        if (mine === subSeq.current) setSubStatuses(list);
      })
      .catch(() => {
        if (mine === subSeq.current) setSubStatuses([]);
      });
  }, []);

  const loadStatus = useCallback(
    (path: string) => {
      if (!path) {
        setStatus(null);
        setSubStatuses([]);
        return;
      }
      const mine = ++seq.current;
      setLoading(true);
      api
        .changesRepoStatus(path)
        .then((s) => {
          if (mine !== seq.current) return; // a newer load already won
          setStatus(s);
          setError(null);
          loadSubStatuses(path, s.has_submodules);
        })
        .catch((e) => {
          if (mine !== seq.current) return;
          setStatus(null);
          setError(String(e));
        })
        .finally(() => {
          if (mine === seq.current) setLoading(false);
        });
    },
    [loadSubStatuses]
  );

  // Switching repos must not show the previous repo's files for a moment.
  useEffect(() => {
    setStatus(null);
    setSubStatuses([]);
    setOpenSubs({});
    setResult(null);
    setAmend(false);
    loadStatus(repoPath);
  }, [repoPath, loadStatus]);

  // After any operation the submodules may have moved too.
  useEffect(() => {
    if (subRefresh === 0) return;
    loadSubStatuses(repoPath, !!status?.has_submodules);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [subRefresh]);

  // Folders change behind the app's back: a terminal commit, a deleted clone.
  useRefreshOnFocus(() => loadStatus(repoPath), 4000);

  // The strip above the tabs shows what this tab knows.
  useEffect(() => {
    onStatus(status, loading);
  }, [status, loading, onStatus]);

  // Keep the elapsed time moving while a job runs.
  useEffect(() => {
    if (!job?.running) return;
    const id = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, [job?.running]);

  // A large-file download can take a long time with nothing on screen moving,
  // which is indistinguishable from the app having hung. git-lfs reports what
  // it is transferring, so ask it rather than leave the person guessing.
  useEffect(() => {
    // Only operations that can download large files. Polling during a push
    // would show the numbers a previous download left behind.
    const downloads = job?.kind === "lfs" || job?.kind === "pull" || job?.kind === "sync";
    if (!job?.running || !downloads || !repoPath) {
      setLfsProgress(null);
      return;
    }
    let stop = false;
    const read = () =>
      api
        .changesLfsProgress(repoPath)
        .then((p) => {
          if (!stop) setLfsProgress(p);
        })
        .catch(() => {
          /* the run may have finished between the poll and the read */
        });
    read();
    const id = setInterval(read, 1000);
    return () => {
      stop = true;
      clearInterval(id);
      setLfsProgress(null);
    };
  }, [job?.running, job?.kind, job?.startedAt, repoPath]);

  // The repository on screen right now, for callbacks that outlive a render:
  // a result that comes back after the person has moved to another
  // repository must not be painted into that one's view.
  const repoRef = useRef(repoPath);
  useEffect(() => {
    repoRef.current = repoPath;
  }, [repoPath]);

  // A finished job is shown by this effect and nowhere else. `job` is the one
  // for the repository on screen, so a job that finished while another
  // repository was showing waits there until the person comes back. Seen
  // jobs are remembered per repository, so returning to one does not replay
  // a result that was already dismissed.
  const seenJob = useRef<Record<string, number>>({});
  useEffect(() => {
    if (!job || job.running || seenJob.current[repoPath] === job.startedAt) return;
    seenJob.current[repoPath] = job.startedAt;
    if (job.result) {
      setResult(job.result);
      // The result carries the status git had after the operation, so a
      // second full status read on top of it would be wasted work.
      if (job.result.status) setStatus(job.result.status);
      else loadStatus(repoPath);
    }
    if (job.error) {
      setError(job.error);
      loadStatus(repoPath);
    }
    setSubRefresh((n) => n + 1);
  }, [job, repoPath, loadStatus]);

  /** Work in another repo (a submodule) without waiting for the scan. */
  const openRepo = (path: string) => onOpenRepo(path);

  const message = drafts[repoPath] ?? "";
  const setMessage = (m: string) => setDrafts((d) => ({ ...d, [repoPath]: m }));
  // What the box shows is what gets committed: with nothing typed, a merge in
  // progress commits with git's own MERGE_MSG rather than an empty message.
  const commitMessage = message || (status?.merge_message ?? "");

  const entries = status?.entries ?? [];
  const dirty = (status?.staged_count ?? 0) + (status?.unstaged_count ?? 0) > 0;

  /**
   * Apply a foreground operation and adopt the status it returns. With a
   * `target` (a submodule's path) the status belongs to that section, and the
   * parent is re-read because its gitlink row just changed too. A refusal the
   * caller named in `opts.quietRefusal` is a question it answers itself (a
   * confirm step), so it gets neither a toast nor a result card.
   */
  const apply = async (key: string, fn: () => Promise<OpResult>, target?: string, opts?: RunOptions) => {
    const forRepo = repoPath;
    setBusy(key);
    setError(null);
    try {
      const r = await fn();
      const quiet = !r.ok && !!opts?.quietRefusal && r.refusal?.code === opts.quietRefusal;
      // Still looking at the repository this ran in? Only then is the view
      // updated; the toast below says what happened either way.
      const stillHere = repoRef.current === forRepo;
      if (!quiet && stillHere) setResult(r);
      if (r.status && stillHere) {
        const s = r.status;
        if (target && target !== forRepo) {
          setSubStatuses((list) => list.map((x) => (x.status.path === target ? { ...x, status: s } : x)));
          loadStatus(forRepo);
        } else {
          setStatus(s);
        }
      }
      if (stillHere) setSubRefresh((n) => n + 1);
      if (r.ok) {
        toast.success(r.headline);
      } else if (quiet) {
        // the caller asks the question in its own dialog
      } else if (r.refusal) {
        toast.error(r.refusal.message);
      } else if (r.advice) {
        toast.error(r.advice.headline);
      }
      return r;
    } catch (e) {
      setError(String(e));
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  /**
   * Long operations survive leaving the page, so they run in the job slot.
   * Painting the result is left to the job effect above, which is keyed to
   * the repository on screen — so a pull that finishes after the person has
   * moved to another repository is shown when they come back, not over the
   * one they are looking at.
   */
  const runLongIn = (repo: string, kind: GitJobKind, label: string, fn: () => Promise<OpResult>) => {
    setResult(null);
    setError(null);
    return runJob(repo, kind, label, fn).then((r) => {
      if (!r) return;
      if (r.ok) toast.success(r.headline);
      else toast.error(r.refusal?.message ?? r.advice?.headline ?? "That didn't work");
    });
  };
  const runLong = (kind: GitJobKind, label: string, fn: () => Promise<OpResult>) => runLongIn(repoPath, kind, label, fn);

  /**
   * `git pull` inside one submodule, on its own branch, with the page's pull
   * settings. It is a foreground operation on that submodule (`apply` with a
   * target): the result carries the submodule's status, never the parent's,
   * and the parent is re-read because its gitlink row moves.
   */
  const pullInside = (repo: string, s: SubmoduleInfo) => {
    const abs = `${repo.replace(/\/+$/, "")}/${s.path}`;
    return apply(
      `pull:${s.path}`,
      () => api.changesPull(abs, pullMode, (status?.uses_lfs ?? false) && pullLfs, pullMode === "rebase" && pullAutostash),
      abs
    );
  };

  const onCommit = async () => {
    const r = await apply("commit", () => api.changesCommit(repoPath, commitMessage, amend));
    if (r?.ok) {
      setDrafts((d) => ({ ...d, [repoPath]: "" }));
      setAmend(false);
    }
  };

  const discardNow = async () => {
    const c = confirmDiscard;
    setConfirmDiscard(null);
    if (!c) return;
    await apply("discard", () => api.changesDiscard(c.target ?? repoPath, c.paths), c.target);
  };

  const running = job?.running ?? false;
  const anyBusy = busy !== null || running;

  // The strip's buttons: fresh closures every render, stored in a ref upstairs.
  useEffect(() => {
    onActions({
      fetch: () =>
        runLong("fetch", "Fetching…", async () => {
          await api.historyFetch(repoPath);
          const s = await api.changesRepoStatus(repoPath);
          return {
            ok: true,
            headline: "Fetched.",
            detail: `${s.behind} commit(s) waiting to be pulled.`,
            status: s,
          };
        }),
      pull: () =>
        status
          ? runLong(
              "pull",
              status.uses_lfs && pullLfs ? `Pulling (${pullMode}) and downloading LFS files…` : `Pulling (${pullMode})…`,
              () => api.changesPull(repoPath, pullMode, status.uses_lfs && pullLfs, pullMode === "rebase" && pullAutostash)
            )
          : undefined,
      push: () => (status ? runLong("push", "Pushing…", () => api.changesPush(repoPath, !status.upstream)) : undefined),
      branches: () => setBranchesFor(repoPath),
      refresh: () => loadStatus(repoPath),
      initialise: (repo, paths) =>
        runLongIn(repo, "submodules", `Initialising ${paths.length} submodule${paths.length === 1 ? "" : "s"}…`, () =>
          api.changesSubmoduleUpdate(repo, paths)
        ),
      updateOne: (repo, sub) => runLongIn(repo, "submodules", `Updating ${sub}…`, () => api.changesSubmoduleUpdate(repo, [sub])),
      pullInside,
      recordPointers: (repo) => apply("record", () => api.changesRecordPointers(repo)),
    });
  });

  // The entries the discard confirmation describes: the parent's, or the submodule's.
  const confirmEntries = confirmDiscard?.target
    ? subStatuses.find((s) => s.status.path === confirmDiscard.target)?.status.entries ?? []
    : entries;

  // Submodules with something to do get a section; the rest are one line.
  const sectionSubs = subStatuses.filter((s) => needsSection(s.status));
  const quietSubs = subStatuses.length - sectionSubs.length;
  const sectionPaths = sectionSubs.map((s) => s.path);
  const isSubOpen = (s: SubmoduleStatus) =>
    openSubs[s.path] ?? (s.status.conflicted_count > 0 || sectionSubs.length <= 2);
  const jumpToSubmodule = (path: string) => {
    setOpenSubs((o) => ({ ...o, [path]: true }));
    // After the body has rendered, so the section lands at the top of the view.
    setTimeout(() => {
      document.getElementById(sectionId(path))?.scrollIntoView({ behavior: "smooth", block: "start" });
    }, 50);
  };

  // The branch picker can serve the parent or one submodule.
  const branchesStatus =
    branchesFor === null
      ? null
      : branchesFor === repoPath
        ? status
        : subStatuses.find((s) => s.status.path === branchesFor)?.status ?? null;

  const refusalCode = result?.refusal?.code;
  const historyAction =
    refusalCode === "already-pushed" || refusalCode === "would-drop-pushed"
      ? {
          label: "Open in History",
          onClick: () => {
            onOpenHistory();
          },
        }
      : undefined;
  // A branch whose upstream has another name (or is a local branch) is not
  // pushed there; the way out is to publish it under its own name.
  const publishAction =
    (refusalCode === "upstream-name-differs" || refusalCode === "local-upstream") && status?.branch
      ? {
          label: `Publish as ${result?.refusal?.message.match(/`([^`]+\/[^`]+)` instead/)?.[1] ?? status.branch}`,
          primary: true,
          onClick: () => runLong("push", "Publishing…", () => api.changesPush(repoPath, true)),
        }
      : undefined;

  return (
    <div className={`space-y-5 ${visible ? "" : "hidden"}`} role="tabpanel" aria-label="Changes">

      {error && (
        <Card className="border-red-500/30 bg-red-500/5">
          <p className="text-sm text-red-300 break-words">{error}</p>
        </Card>
      )}

      {running && job && (
        <Card className="border-sky-500/30 bg-sky-500/5">
          <div className="flex items-center gap-2 text-sm text-sky-200">
            <Loader2 size={15} className="animate-spin shrink-0" />
            <span>{job.label}</span>
            <span className="text-xs text-sky-300/70">{elapsedLabel(job.startedAt)}</span>
            {lfsProgress && (
              <span className="min-w-0 truncate text-xs text-sky-300/90" title={lfsProgress.file}>
                {/* git-lfs counts bytes per file, not across the transfer, so
                    they are shown as that one file's progress. */}
                · {lfsProgress.done} of {lfsProgress.total} files · {lfsProgress.file} (
                {formatBytes(lfsProgress.bytes)} of {formatBytes(lfsProgress.total_bytes)})
              </span>
            )}
            <span className="text-xs text-zinc-500 ml-auto">
              keeps running if you leave this page
            </span>
          </div>
        </Card>
      )}

      {!repoPath && (
        <Card>
          <p className="text-sm text-zinc-400">Pick a repository above to see what has changed in it.</p>
        </Card>
      )}

      {status && (
        <>
          {status.sync && (
            <SyncPausedCard
              sync={status.sync}
              status={status}
              busy={anyBusy}
              onContinue={() =>
                runLong("sync", "Continuing the sync…", () => api.changesSyncContinue(repoPath))
              }
              onAbort={() => runLong("sync", "Aborting the sync…", () => api.changesSyncAbort(repoPath))}
              onOpenRoot={() => status.sync && openRepo(status.sync.root)}
            />
          )}

          {status.operation && !status.sync && (
            <Card className="border-amber-500/30 bg-amber-500/5">
              <div className="flex flex-wrap items-center gap-3">
                <AlertTriangle size={16} className="text-amber-400 shrink-0" />
                <div className="min-w-0 flex-1">
                  <p className="text-sm text-amber-100">{status.operation.label}</p>
                  <p className="text-xs text-amber-200/70">
                    {status.operation.detail || "Finish it, or abort to go back."}
                  </p>
                </div>
                {status.operation.continue_command && status.operation.kind !== "merge" && (
                  <Button
                    size="sm"
                    variant="primary"
                    disabled={anyBusy || status.conflicted_count > 0}
                    onClick={() => apply("continue", () => api.changesContinue(repoPath))}
                    title={
                      status.conflicted_count > 0
                        ? "Resolve and stage every conflicted file first"
                        : status.operation.continue_command
                    }
                  >
                    <Play size={13} />
                    Continue
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={anyBusy}
                  onClick={() => apply("abort", () => api.changesAbort(repoPath))}
                  title={status.operation.abort_command}
                >
                  <CircleSlash size={13} />
                  Abort
                </Button>
              </div>
            </Card>
          )}

          {/* How a pull behaves, and the tidy-up strip. The branch line and the
              Fetch / Pull / Push buttons live on the strip above the tabs. */}
          <TidyUpStrip
            status={status}
            busy={anyBusy}
            dirty={dirty}
            pullMode={pullMode}
            onPullMode={onPullMode}
            pullLfs={pullLfs}
            onPullLfs={onPullLfs}
            pullAutostash={pullAutostash}
            onPullAutostash={onPullAutostash}
            onUndoCommit={() => apply("undo", () => api.changesUndoCommit(repoPath))}
            onStash={() => setStashOpen(true)}
            onReset={() => setResetOpen(true)}
            onDiscardAll={() => setDiscardAllOpen(true)}
          />

          {!status.unborn && !status.sync && (
            <SyncCard
              repoPath={repoPath}
              status={status}
              busy={anyBusy}
              onSync={(stash, bundles, fingerprint) =>
                runLong("sync", "Syncing — fetching, rebasing, aligning submodules…", () =>
                  api.changesSyncRun(repoPath, stash, bundles, fingerprint)
                )
              }
              onSwitchBranch={() => setBranchesFor(repoPath)}
            />
          )}

          {result && (
            <ResultCard
              result={result}
              busy={anyBusy}
              onDismiss={() => {
                setResult(null);
                clearJob(repoPath);
              }}
              onRecordPointers={() => apply("record", () => api.changesRecordPointers(repoPath))}
              extraAction={historyAction ?? publishAction}
            />
          )}

          {/* items-start only in row mode: in the stacked (column) layout it is the
              cross axis, so it would size each column to its content and let a long
              file path widen the whole page. */}
          <div className="flex flex-col lg:flex-row gap-4 lg:items-start">
            <div className="flex-1 min-w-0 space-y-3">
              <TreeLists
                status={status}
                busy={anyBusy}
                onOpen={setDiffFor}
                onStage={(paths) => apply("stage", () => api.changesStage(repoPath, paths))}
                onUnstage={(paths) => apply("unstage", () => api.changesUnstage(repoPath, paths))}
                onDiscard={(paths, variant) => setConfirmDiscard({ paths, variant })}
                onResolveSide={(paths, side) =>
                  apply("resolve", () => api.changesResolveSide(repoPath, paths, side))
                }
                jumpableSubmodules={sectionPaths}
                onJumpToSubmodule={jumpToSubmodule}
              />

              <CommitBox
                status={status}
                message={commitMessage}
                onMessage={setMessage}
                amend={amend}
                onAmend={setAmend}
                busy={anyBusy}
                onCommit={onCommit}
                onSwitchBranch={() => setBranchesFor(repoPath)}
              />

              {sectionSubs.map((s) => (
                <SubmoduleSection
                  key={s.path}
                  sub={s}
                  parentRepoPath={repoPath}
                  parentStatus={status}
                  open={isSubOpen(s)}
                  onToggle={() => setOpenSubs((o) => ({ ...o, [s.path]: !isSubOpen(s) }))}
                  busy={anyBusy}
                  draft={drafts[s.status.path] ?? ""}
                  onDraft={(m) => setDrafts((d) => ({ ...d, [s.status.path]: m }))}
                  apply={apply}
                  onOpenRepo={openRepo}
                  onDiscard={(paths, variant) => setConfirmDiscard({ paths, variant, target: s.status.path })}
                  onSwitchBranch={() => setBranchesFor(s.status.path)}
                />
              ))}
              {quietSubs > 0 && (
                <p className="text-xs text-zinc-500">
                  {quietSubs} {sectionSubs.length > 0 ? "other " : ""}submodule{quietSubs === 1 ? "" : "s"}{" "}
                  {quietSubs === 1 ? "has" : "have"} nothing to commit.
                </p>
              )}
            </div>

            <div className="w-full lg:w-80 shrink-0 space-y-3">
              <PushAccessCard
                repoPath={repoPath}
                push={status.push}
                onChanged={(p) => {
                  // A mode change that lands after a switch to another
                  // repository must not carry this status over to it.
                  if (repoRef.current !== repoPath) return;
                  setStatus((s) => (s ? { ...s, push: p } : s));
                }}
              />

              {status.uses_lfs && (
                <LfsCard
                  repoPath={repoPath}
                  busy={anyBusy}
                  refreshKey={subRefresh}
                  onPull={() =>
                    runLong("lfs", "Downloading LFS files…", () => api.changesLfsPull(repoPath))
                  }
                  onBrowse={() => setLfsBrowser(true)}
                />
              )}

              <SubmodulesCard
                repoPath={repoPath}
                busy={anyBusy}
                refreshKey={subRefresh}
                onOpenRepo={openRepo}
                onUpdate={(paths) =>
                  runLong("submodules", `Initialising ${paths.length} submodule${paths.length === 1 ? "" : "s"}…`, () =>
                    api.changesSubmoduleUpdate(repoPath, paths)
                  )
                }
                onPullInside={(s) => void pullInside(repoPath, s)}
                onUpdateOne={(p) => void runLong("submodules", `Updating ${p}…`, () => api.changesSubmoduleUpdate(repoPath, [p]))}
                sectionPaths={sectionPaths}
                onJumpToSection={jumpToSubmodule}
              />

              <StashesCard
                repoPath={repoPath}
                status={status}
                busy={anyBusy}
                refreshKey={subRefresh}
                run={(key, fn) => apply(key, fn)}
              />
            </div>
          </div>
        </>
      )}

      {diffFor && status && (
        <DiffPanel repoPath={repoPath} entry={diffFor} onClose={() => setDiffFor(null)} />
      )}

      <DiscardConfirmModal
        open={confirmDiscard !== null}
        paths={confirmDiscard?.paths ?? []}
        entries={confirmEntries}
        variant={confirmDiscard?.variant ?? "changes"}
        onCancel={() => setConfirmDiscard(null)}
        onConfirm={discardNow}
      />

      {status && (
        <>
          <StashModal
            open={stashOpen}
            status={status}
            onCancel={() => setStashOpen(false)}
            onConfirm={(msg, includeUntracked) => {
              setStashOpen(false);
              void apply("stash", () => api.changesStashPush(repoPath, msg, includeUntracked));
            }}
          />
          <ResetConfirmModal
            open={resetOpen}
            status={status}
            onCancel={() => setResetOpen(false)}
            onConfirm={(stashFirst) => {
              setResetOpen(false);
              void apply("reset", () => api.changesReset(repoPath, "@{u}", "hard", stashFirst));
            }}
          />
          <LfsFilesModal
            open={lfsBrowser}
            repoPath={repoPath}
            busy={anyBusy}
            onClose={() => setLfsBrowser(false)}
            onDownload={(paths) =>
              runLong(
                "lfs",
                `Downloading ${paths.length === 1 ? paths[0] : `${paths.length} selected paths`}…`,
                () => api.changesLfsPullPaths(repoPath, paths)
              )
            }
            onDownloadAll={() =>
              runLong("lfs", "Downloading LFS files…", () => api.changesLfsPull(repoPath))
            }
          />
          <DiscardAllModal
            open={discardAllOpen}
            status={status}
            onCancel={() => setDiscardAllOpen(false)}
            onConfirm={(includeUntracked, stashFirst) => {
              setDiscardAllOpen(false);
              void apply("discard", () => api.changesDiscardAll(repoPath, includeUntracked, stashFirst));
            }}
          />
        </>
      )}

      {branchesFor !== null && branchesStatus && (
        <BranchesModal
          open
          onClose={() => setBranchesFor(null)}
          repoPath={branchesFor}
          status={branchesStatus}
          busy={anyBusy}
          run={(key, fn, opts) => apply(key, fn, branchesFor === repoPath ? undefined : branchesFor, opts)}
        />
      )}

      {/* tick keeps the elapsed timer honest without re-rendering anything else */}
      <span className="hidden">{tick}</span>
    </div>
  );
}
