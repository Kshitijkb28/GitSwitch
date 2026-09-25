import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { RefreshCw, Loader2, AlertTriangle, CircleSlash, Play } from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import { TreeLists, type DiscardVariant } from "../components/changes/TreeLists";
import { CommitBox } from "../components/changes/CommitBox";
import { DiffPanel } from "../components/changes/DiffPanel";
import { BranchCard } from "../components/changes/BranchCard";
import { BranchesModal, type RunOptions } from "../components/changes/BranchesModal";
import { PushAccessCard } from "../components/changes/PushAccessCard";
import { SubmodulesCard } from "../components/changes/SubmodulesCard";
import { SubmoduleSection, needsSection, sectionId } from "../components/changes/SubmoduleSection";
import { LfsCard } from "../components/changes/LfsCard";
import { LfsFilesModal, formatBytes } from "../components/changes/LfsFilesModal";
import { StashesCard } from "../components/changes/StashesCard";
import { StashModal } from "../components/changes/StashModal";
import { SyncCard, SyncPausedCard } from "../components/changes/SyncCard";
import { ResultCard } from "../components/changes/ResultCard";
import { DiscardConfirmModal } from "../components/changes/DiscardConfirmModal";
import { ResetConfirmModal } from "../components/changes/ResetConfirmModal";
import { DiscardAllModal } from "../components/changes/DiscardAllModal";
import { baseName } from "../lib/paths";
import { usePersistedState, writePersisted } from "../lib/persist";
import { useRefreshOnFocus } from "../lib/focus";
import { elapsedLabel, runJob, useGitJob, clearJob, type GitJobKind } from "../lib/gitJobs";
import * as api from "../lib/api";
import type { ChangeEntry, OpResult, PullMode, RepoRef, RepoStatus, SubmoduleStatus } from "../lib/api";

type DiscardRequest = { paths: string[]; variant: DiscardVariant; target?: string };

export function Changes() {
  const toast = useToast();
  const navigate = useNavigate();
  const [repos, setRepos] = useState<RepoRef[]>([]);
  const [account, setAccount] = usePersistedState("changes.account", ""); // "" = all
  const [repoPath, setRepoPath] = usePersistedState("changes.repo", "");
  const [pullMode, setPullMode] = usePersistedState<PullMode>("changes.pullMode", "ff-only");
  const [drafts, setDrafts] = usePersistedState<Record<string, string>>("changes.drafts", {});
  // On by default: with LFS filters configured git already downloads large
  // files on pull, so this makes every repo behave the way people expect.
  const [pullLfs, setPullLfs] = usePersistedState("changes.pullLfs", true);
  // Off by default: stashing around a rebase can leave conflicts behind, so
  // it is a choice, not a surprise.
  const [pullAutostash, setPullAutostash] = usePersistedState("changes.pullAutostash", false);

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
  const [extraRepos, setExtraRepos] = useState<string[]>([]);

  const job = useGitJob(repoPath);
  const seq = useRef(0);
  const subSeq = useRef(0);

  const accounts = useMemo(
    () =>
      Array.from(
        new Map(repos.map((r) => [r.profile_id, { id: r.profile_id, name: r.profile_name }])).values()
      ),
    [repos]
  );
  const visibleRepos = useMemo(
    () => (account ? repos.filter((r) => r.profile_id === account) : repos),
    [account, repos]
  );

  const loadRepos = useCallback(() => {
    api
      .historyListRepos()
      .then(setRepos)
      .catch((e) => setError(String(e)));
  }, []);

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

  useEffect(loadRepos, [loadRepos]);

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

  // If the chosen account doesn't own the current repo, move to one it does.
  useEffect(() => {
    if (!account || !repos.length) return;
    const owned = repos.filter((r) => r.profile_id === account);
    if (owned.length && !owned.some((r) => r.path === repoPath)) {
      setRepoPath(owned[0].path);
    }
  }, [account, repos, repoPath, setRepoPath]);

  // Folders change behind the app's back: a terminal commit, a deleted clone.
  useRefreshOnFocus(() => loadStatus(repoPath), 4000);
  useRefreshOnFocus(loadRepos, 15000);

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

  // A job that finished while we were on another page: show it and refresh.
  const seenJob = useRef<number>(0);
  useEffect(() => {
    if (!job || job.running || job.startedAt === seenJob.current) return;
    seenJob.current = job.startedAt;
    if (job.result) setResult(job.result);
    if (job.error) setError(job.error);
    loadStatus(repoPath);
  }, [job, repoPath, loadStatus]);

  /** Work in another repo (a submodule) without waiting for the scan. */
  const openRepo = (path: string) => {
    setExtraRepos((list) => (list.includes(path) ? list : [...list, path]));
    setRepoPath(path);
  };

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
    setBusy(key);
    setError(null);
    try {
      const r = await fn();
      const quiet = !r.ok && !!opts?.quietRefusal && r.refusal?.code === opts.quietRefusal;
      if (!quiet) setResult(r);
      if (r.status) {
        const s = r.status;
        if (target && target !== repoPath) {
          setSubStatuses((list) => list.map((x) => (x.status.path === target ? { ...x, status: s } : x)));
          loadStatus(repoPath);
        } else {
          setStatus(s);
        }
      }
      setSubRefresh((n) => n + 1);
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

  /** Long operations survive leaving the page, so they run in the job slot. */
  const runLong = (kind: GitJobKind, label: string, fn: () => Promise<OpResult>) => {
    setResult(null);
    setError(null);
    void runJob(repoPath, kind, label, fn).then((r) => {
      if (!r) return;
      setResult(r);
      if (r.status) setStatus(r.status);
      if (r.ok) toast.success(r.headline);
      else toast.error(r.refusal?.message ?? r.advice?.headline ?? "That didn't work");
      loadStatus(repoPath);
      setSubRefresh((n) => n + 1);
    });
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
            writePersisted("history.repo", repoPath);
            navigate("/history");
          },
        }
      : undefined;

  return (
    <div className="max-w-6xl mx-auto space-y-5">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">Changes</h1>
          <p className="text-sm text-zinc-400 mt-0.5">
            Stage, commit, pull and push — as the account this folder belongs to.
          </p>
        </div>
        <div className="flex items-end gap-2 flex-wrap">
          <div className="w-44">
            <label className="block text-xs text-zinc-500 mb-1">Account</label>
            <Select
              value={account}
              onChange={setAccount}
              placeholder="All accounts"
              options={[
                { value: "", label: "All accounts" },
                ...accounts.map((a) => ({ value: a.id, label: a.name })),
              ]}
            />
          </div>
          <div className="w-64">
            <label className="block text-xs text-zinc-500 mb-1">Repository</label>
            <Select
              value={repoPath}
              onChange={setRepoPath}
              placeholder={visibleRepos.length ? "Pick a repository" : "No repositories found"}
              options={[
                ...visibleRepos.map((r) => ({
                  value: r.path,
                  label: account ? r.name : `${r.name}  ·  ${r.profile_name}`,
                })),
                ...extraRepos
                  .filter((p) => !visibleRepos.some((r) => r.path === p))
                  .map((p) => ({ value: p, label: baseName(p) })),
              ]}
            />
          </div>
          <Button
            variant="secondary"
            size="sm"
            disabled={!repoPath || loading}
            onClick={() => {
              loadRepos();
              loadStatus(repoPath);
            }}
            className="min-w-[6.5rem]"
          >
            {loading ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <RefreshCw size={14} />
            )}
            {loading ? "Reading…" : "Refresh"}
          </Button>
        </div>
      </header>

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
          <p className="text-sm text-zinc-400">
            Pick a repository to see what has changed in it.
          </p>
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

          {/* Where this branch stands, the two ways to move it, and the tidy-up strip. */}
          <BranchCard
            repoPath={repoPath}
            status={status}
            busy={anyBusy}
            dirty={dirty}
            pullMode={pullMode}
            onPullMode={setPullMode}
            pullLfs={pullLfs}
            onPullLfs={setPullLfs}
            pullAutostash={pullAutostash}
            onPullAutostash={setPullAutostash}
            onFetch={() =>
              runLong("fetch", "Fetching…", async () => {
                await api.historyFetch(repoPath);
                const s = await api.changesRepoStatus(repoPath);
                return {
                  ok: true,
                  headline: "Fetched.",
                  detail: `${s.behind} commit(s) waiting to be pulled.`,
                  status: s,
                };
              })
            }
            onPull={() =>
              runLong(
                "pull",
                status.uses_lfs && pullLfs
                  ? `Pulling (${pullMode}) and downloading LFS files…`
                  : `Pulling (${pullMode})…`,
                () =>
                  api.changesPull(
                    repoPath,
                    pullMode,
                    status.uses_lfs && pullLfs,
                    pullMode === "rebase" && pullAutostash
                  )
              )
            }
            onPush={() =>
              runLong("push", "Pushing…", () => api.changesPush(repoPath, !status.upstream))
            }
            onBranches={() => setBranchesFor(repoPath)}
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
              extraAction={historyAction}
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
                onChanged={(p) => setStatus({ ...status, push: p })}
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
                onUpdate={() =>
                  runLong("submodules", "Updating submodules…", () =>
                    api.changesSubmoduleUpdate(repoPath)
                  )
                }
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
