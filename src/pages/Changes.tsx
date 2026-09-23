import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, Loader2, ArrowUp, ArrowDown, GitBranch, Upload, Download, CheckCircle2, AlertTriangle, XCircle, FilePlus2, FileEdit, ShieldAlert, Ban, CircleSlash, Play } from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Checkbox } from "../components/Checkbox";
import { Select } from "../components/Select";
import { Modal } from "../components/Modal";
import { useToast } from "../components/Toast";
import { ChangesList } from "../components/changes/ChangesList";
import { CommitBox } from "../components/changes/CommitBox";
import { DiffPanel } from "../components/changes/DiffPanel";
import { PullControl, whyDisabled } from "../components/changes/PullControl";
import { PushAccessCard } from "../components/changes/PushAccessCard";
import { SubmodulesCard } from "../components/changes/SubmodulesCard";
import { LfsCard } from "../components/changes/LfsCard";
import { SyncCard, SyncOutcomeView, SyncPausedCard } from "../components/changes/SyncCard";
import { baseName } from "../lib/paths";
import { usePersistedState } from "../lib/persist";
import { useRefreshOnFocus } from "../lib/focus";
import { elapsedLabel, runJob, useGitJob, clearJob, type GitJobKind } from "../lib/gitJobs";
import * as api from "../lib/api";
import type { ChangeEntry, OpResult, PullMode, RepoRef, RepoStatus } from "../lib/api";

/** "4 minutes ago" for the last fetch, so staleness is readable at a glance. */
function agoLabel(secs: number | null): string {
  if (secs === null) return "never fetched";
  if (secs < 90) return "fetched just now";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `fetched ${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 48) return `fetched ${hours}h ago`;
  return `fetched ${Math.round(hours / 24)} days ago`;
}

export function Changes() {
  const toast = useToast();
  const [repos, setRepos] = useState<RepoRef[]>([]);
  const [account, setAccount] = usePersistedState("changes.account", ""); // "" = all
  const [repoPath, setRepoPath] = usePersistedState("changes.repo", "");
  const [pullMode, setPullMode] = usePersistedState<PullMode>("changes.pullMode", "ff-only");
  const [drafts, setDrafts] = usePersistedState<Record<string, string>>("changes.drafts", {});
  // On by default: with LFS filters configured git already downloads large
  // files on pull, so this makes every repo behave the way people expect.
  const [pullLfs, setPullLfs] = usePersistedState("changes.pullLfs", true);

  // Deliberately not persisted: amending must be a fresh decision every time,
  // and status/results must always be re-read rather than restored.
  const [amend, setAmend] = useState(false);
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<OpResult | null>(null);
  const [diffFor, setDiffFor] = useState<ChangeEntry | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState<string[] | null>(null);
  const [tick, setTick] = useState(0);
  const [subRefresh, setSubRefresh] = useState(0);
  const [extraRepos, setExtraRepos] = useState<string[]>([]);

  const job = useGitJob(repoPath);
  const seq = useRef(0);

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

  const loadStatus = useCallback(
    (path: string) => {
      if (!path) {
        setStatus(null);
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
    []
  );

  useEffect(loadRepos, [loadRepos]);

  // Switching repos must not show the previous repo's files for a moment.
  useEffect(() => {
    setStatus(null);
    setResult(null);
    setAmend(false);
    loadStatus(repoPath);
  }, [repoPath, loadStatus]);

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

  const entries = status?.entries ?? [];
  const conflicted = entries.filter((e) => e.kind === "conflicted");
  const staged = entries.filter((e) => e.kind === "tracked" && e.staged !== ".");
  const unstaged = entries.filter((e) => e.kind === "tracked" && e.unstaged !== ".");
  const untracked = entries.filter((e) => e.kind === "untracked");
  const dirty = (status?.staged_count ?? 0) + (status?.unstaged_count ?? 0) > 0;

  /** Apply a foreground operation and adopt the status it returns. */
  const apply = async (key: string, fn: () => Promise<OpResult>) => {
    setBusy(key);
    setError(null);
    try {
      const r = await fn();
      setResult(r);
      if (r.status) setStatus(r.status);
      setSubRefresh((n) => n + 1);
      if (r.ok) {
        toast.success(r.headline);
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
    const r = await apply("commit", () => api.changesCommit(repoPath, message, amend));
    if (r?.ok) {
      setDrafts((d) => ({ ...d, [repoPath]: "" }));
      setAmend(false);
    }
  };

  const discardNow = async (paths: string[]) => {
    setConfirmDiscard(null);
    await apply("discard", () => api.changesDiscard(repoPath, paths));
  };

  const pullBlocked = status
    ? whyDisabled(pullMode, status.ahead, status.behind, dirty)
    : null;
  const running = job?.running ?? false;
  const anyBusy = busy !== null || running;

  const confirmEntries = (confirmDiscard ?? []).map((p) =>
    entries.find((e) => e.path === p)
  );
  const willDelete = confirmEntries.filter(
    (e) => e && (e.kind === "untracked" || e.staged === "A")
  ).length;

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

          {/* Sync: where this branch stands, and the two ways to move it. */}
          <Card>
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div className="min-w-0 space-y-2">
                <div className="flex items-center gap-2 flex-wrap">
                  <GitBranch size={15} className="text-zinc-400 shrink-0" />
                  <span className="font-medium text-zinc-100">
                    {status.detached ? "detached HEAD" : (status.branch ?? "no branch")}
                  </span>
                  {status.upstream ? (
                    <span className="text-xs text-zinc-500">→ {status.upstream}</span>
                  ) : (
                    <span className="text-xs text-amber-400/90">not published yet</span>
                  )}
                  {status.ahead > 0 && (
                    <span className="inline-flex items-center gap-0.5 text-xs text-emerald-400">
                      <ArrowUp size={12} />
                      {status.ahead}
                    </span>
                  )}
                  {status.behind > 0 && (
                    <span className="inline-flex items-center gap-0.5 text-xs text-sky-400">
                      <ArrowDown size={12} />
                      {status.behind}
                    </span>
                  )}
                  <span className="text-xs text-zinc-600">
                    {agoLabel(status.last_fetch_secs)}
                  </span>
                </div>
                <PullControl
                  mode={pullMode}
                  onMode={setPullMode}
                  ahead={status.ahead}
                  behind={status.behind}
                  dirty={dirty}
                  upstream={status.upstream}
                />
                {status.uses_lfs && (
                  <label
                    className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer"
                    title="A plain pull only downloads large files when this repository's LFS filters are set up — otherwise they arrive as pointer stubs"
                  >
                    <Checkbox
                      checked={pullLfs}
                      onChange={setPullLfs}
                      className="mt-0.5"
                      aria-label="Also download LFS files when pulling"
                    />
                    <span className="leading-relaxed">
                      Also download Git LFS files
                      <span className="text-zinc-500">
                        {" "}
                        — otherwise large files arrive as pointer stubs
                      </span>
                    </span>
                  </label>
                )}
              </div>

              <div className="flex items-center gap-2 flex-wrap">
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={anyBusy || !repoPath}
                  onClick={() =>
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
                  className="min-w-[5.5rem]"
                >
                  <RefreshCw size={13} />
                  Fetch
                </Button>
                <Button
                  size="sm"
                  variant={status.behind > 0 ? "primary" : "secondary"}
                  disabled={anyBusy || !status.upstream || pullBlocked !== null}
                  title={pullBlocked ?? undefined}
                  onClick={() =>
                    runLong(
                      "pull",
                      status.uses_lfs && pullLfs
                        ? `Pulling (${pullMode}) and downloading LFS files…`
                        : `Pulling (${pullMode})…`,
                      () => api.changesPull(repoPath, pullMode, status.uses_lfs && pullLfs)
                    )
                  }
                  className="min-w-[8.5rem]"
                >
                  <Download size={13} />
                  Pull ({pullMode === "ff-only" ? "fast-forward" : pullMode})
                </Button>
                <Button
                  size="sm"
                  variant={status.behind > 0 ? "secondary" : "primary"}
                  disabled={anyBusy || status.push.blocked || status.detached || status.unborn}
                  title={status.push.blocked ? status.push.reason : undefined}
                  onClick={() =>
                    runLong("push", "Pushing…", () =>
                      api.changesPush(repoPath, !status.upstream)
                    )
                  }
                  className="min-w-[8.5rem]"
                >
                  {status.push.blocked ? <Ban size={13} /> : <Upload size={13} />}
                  {status.push.lock.locked
                    ? "Push locked"
                    : status.push.blocked
                    ? "Push blocked"
                    : status.upstream
                      ? `Push${status.ahead ? ` (${status.ahead})` : ""}`
                      : "Publish branch"}
                </Button>
              </div>
            </div>
          </Card>

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
            />
          )}

          {result && (
            <Card
              className={
                result.ok ? "border-emerald-500/30 bg-emerald-500/5" : "border-red-500/30 bg-red-500/5"
              }
            >
              <div className="flex items-start gap-2">
                {result.ok ? (
                  <CheckCircle2 size={16} className="text-emerald-400 shrink-0 mt-0.5" />
                ) : (
                  <XCircle size={16} className="text-red-400 shrink-0 mt-0.5" />
                )}
                <div className="min-w-0 flex-1 space-y-1">
                  <p className={`text-sm ${result.ok ? "text-emerald-100" : "text-red-100"}`}>
                    {result.headline}
                  </p>
                  {result.detail && (
                    <p className="text-xs text-zinc-400 break-words">{result.detail}</p>
                  )}
                  {result.advice?.guidance && (
                    <p className="text-xs text-zinc-300 break-words">{result.advice.guidance}</p>
                  )}
                  {result.pull?.recovery && (
                    <p className="text-xs text-zinc-500 font-mono break-all">
                      undo: {result.pull.recovery}
                    </p>
                  )}
                  {result.submodules && result.submodules.listed > 0 && (
                    <p className="text-xs text-zinc-400">
                      Submodules: {result.submodules.downloaded} of {result.submodules.listed} up to date
                      {result.submodules.failed_paths.length > 0 && ` — failed: ${result.submodules.failed_paths.join(", ")}`}.
                    </p>
                  )}
                  {result.lfs && result.lfs.uses_lfs && (
                    <p className="text-xs text-zinc-400">Git LFS: {result.lfs.summary}</p>
                  )}
                  {result.sync && (
                    <SyncOutcomeView
                      outcome={result.sync}
                      busy={anyBusy}
                      onRecordPointers={() =>
                        apply("record", () => api.changesRecordPointers(repoPath))
                      }
                    />
                  )}
                  {result.advice?.git_said && (
                    <details>
                      <summary className="text-xs text-zinc-500 cursor-pointer hover:text-zinc-400 select-none">
                        what git said
                      </summary>
                      <pre className="mt-1 text-xs text-zinc-500 whitespace-pre-wrap break-words">
                        {result.advice.git_said}
                      </pre>
                    </details>
                  )}
                </div>
                <button
                  onClick={() => {
                    setResult(null);
                    clearJob(repoPath);
                  }}
                  className="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer shrink-0"
                >
                  dismiss
                </button>
              </div>
            </Card>
          )}

          {/* items-start only in row mode: in the stacked (column) layout it is the
              cross axis, so it would size each column to its content and let a long
              file path widen the whole page. */}
          <div className="flex flex-col lg:flex-row gap-4 lg:items-start">
            <div className="flex-1 min-w-0 space-y-3">
              {conflicted.length > 0 && (
                <ChangesList
                  title="Conflicts"
                  icon={<ShieldAlert size={14} />}
                  accent="danger"
                  entries={conflicted}
                  side="unstaged"
                  busy={anyBusy}
                  onOpen={setDiffFor}
                  onStage={(paths) =>
                    apply("stage", () => api.changesStage(repoPath, paths))
                  }
                />
              )}

              <ChangesList
                title="Staged"
                icon={<FileEdit size={14} />}
                entries={staged}
                side="staged"
                busy={anyBusy}
                emptyText="Nothing staged yet."
                headerAction={
                  staged.length > 0 ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={anyBusy}
                      onClick={() =>
                        apply("unstage", () =>
                          api.changesUnstage(repoPath, staged.map((e) => e.path))
                        )
                      }
                    >
                      Unstage all
                    </Button>
                  ) : undefined
                }
                onOpen={setDiffFor}
                onUnstage={(paths) => apply("unstage", () => api.changesUnstage(repoPath, paths))}
              />

              <ChangesList
                title="Not staged"
                icon={<FileEdit size={14} />}
                entries={unstaged}
                side="unstaged"
                busy={anyBusy}
                emptyText="No unstaged changes."
                headerAction={
                  unstaged.length > 0 ? (
                    <div className="flex gap-1">
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={anyBusy}
                        onClick={() => setConfirmDiscard(unstaged.map((e) => e.path))}
                        className="hover:text-red-400"
                      >
                        Discard all
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={anyBusy}
                        onClick={() =>
                          apply("stage", () =>
                            api.changesStage(repoPath, unstaged.map((e) => e.path))
                          )
                        }
                      >
                        Stage all
                      </Button>
                    </div>
                  ) : undefined
                }
                onOpen={setDiffFor}
                onStage={(paths) => apply("stage", () => api.changesStage(repoPath, paths))}
                onDiscard={(paths) => setConfirmDiscard(paths)}
              />

              <ChangesList
                title="Untracked"
                icon={<FilePlus2 size={14} />}
                entries={untracked}
                side="unstaged"
                busy={anyBusy}
                emptyText="No new files."
                headerAction={
                  untracked.length > 0 ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={anyBusy}
                      onClick={() =>
                        apply("stage", () =>
                          api.changesStage(repoPath, untracked.map((e) => e.path))
                        )
                      }
                    >
                      Stage all
                    </Button>
                  ) : undefined
                }
                onOpen={setDiffFor}
                onStage={(paths) => apply("stage", () => api.changesStage(repoPath, paths))}
                onDiscard={(paths) => setConfirmDiscard(paths)}
              />

              {status.untracked_truncated && (
                <p className="text-xs text-amber-400/90">
                  More than 2000 untracked files — only the first 2000 are shown. A
                  .gitignore would make this folder much easier to work with.
                </p>
              )}

              <CommitBox
                status={status}
                message={message || (status.merge_message ?? "")}
                onMessage={setMessage}
                amend={amend}
                onAmend={setAmend}
                busy={anyBusy}
                onCommit={onCommit}
              />
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
              />

              {status.stash_count > 0 && (
                <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-3">
                  <p className="text-xs text-zinc-400">
                    {status.stash_count} stash{status.stash_count === 1 ? "" : "es"} in this
                    repository.
                  </p>
                </div>
              )}
            </div>
          </div>
        </>
      )}

      {diffFor && status && (
        <DiffPanel repoPath={repoPath} entry={diffFor} onClose={() => setDiffFor(null)} />
      )}

      <Modal
        open={confirmDiscard !== null}
        onClose={() => setConfirmDiscard(null)}
        title="Discard these changes?"
      >
        <div className="space-y-3">
          <p className="text-sm text-zinc-300">
            {confirmDiscard?.length === 1
              ? "This throws away the changes in 1 file."
              : `This throws away the changes in ${confirmDiscard?.length ?? 0} files.`}{" "}
            <span className="text-red-300">It cannot be undone.</span>
          </p>
          {willDelete > 0 && (
            <p className="text-sm text-amber-300">
              {willDelete} of them {willDelete === 1 ? "is a new file and will be" : "are new files and will be"}{" "}
              deleted from disk.
            </p>
          )}
          <ul className="max-h-40 overflow-y-auto text-xs font-mono text-zinc-400 space-y-0.5">
            {(confirmDiscard ?? []).slice(0, 50).map((p) => (
              <li key={p} className="truncate">
                {p}
              </li>
            ))}
            {(confirmDiscard?.length ?? 0) > 50 && (
              <li className="text-zinc-500">…and {(confirmDiscard?.length ?? 0) - 50} more</li>
            )}
          </ul>
          <div className="flex justify-end gap-2">
            <Button variant="secondary" size="sm" onClick={() => setConfirmDiscard(null)}>
              Keep them
            </Button>
            <Button
              variant="danger"
              size="sm"
              onClick={() => discardNow(confirmDiscard ?? [])}
            >
              Discard
            </Button>
          </div>
        </div>
      </Modal>

      {/* tick keeps the elapsed timer honest without re-rendering anything else */}
      <span className="hidden">{tick}</span>
    </div>
  );
}
