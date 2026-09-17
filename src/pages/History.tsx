import { useState, useEffect, useRef, useCallback } from "react";
import {
  GitBranch,
  GitMerge,
  Loader2,
  ChevronLeft,
  ChevronRight,
  Search,
  RefreshCw,
  CloudDownload,
  CheckCircle2,
  FileText,
  ShieldCheck,
  X,
} from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Checkbox } from "../components/Checkbox";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Input } from "../components/Input";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import { usePersistedState } from "../lib/persist";
import * as api from "../lib/api";

const PAGE_SIZE = 50;
const ROW_H = 46;
const LANE_W = 16;
const LANE_COLORS = [
  "#34d399", "#60a5fa", "#f472b6", "#fbbf24",
  "#a78bfa", "#22d3ee", "#fb7185", "#4ade80",
];
const laneColor = (l: number) => LANE_COLORS[l % LANE_COLORS.length];
const laneX = (l: number) => l * LANE_W + 10;

/** How stale the ahead/behind numbers are — they're only as good as the last fetch. */
function fetchAgo(secs: number | null): string {
  if (secs === null) return "never fetched — counts may be stale";
  if (secs < 90) return "fetched just now";
  if (secs < 3600) return `fetched ${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `fetched ${Math.floor(secs / 3600)}h ago`;
  return `fetched ${Math.floor(secs / 86400)}d ago`;
}

/** One row of the commit graph: rails passing through, the dot, and the
 *  connectors that fan out to a merge's parents. Mirrors `git log --graph`. */
function GraphCell({ c, maxLane }: { c: api.HistoryCommit; maxLane: number }) {
  const width = (maxLane + 1) * LANE_W + 10;
  const mid = ROW_H / 2;
  const continues = c.parent_lanes.includes(c.lane);

  return (
    <svg width={width} height={ROW_H} className="shrink-0 overflow-visible">
      {c.active_lanes
        .filter((l) => l !== c.lane)
        .map((l) => (
          <line
            key={`rail-${l}`}
            x1={laneX(l)} y1={0} x2={laneX(l)} y2={ROW_H}
            stroke={laneColor(l)} strokeWidth={2} opacity={0.55}
          />
        ))}
      {/* own lane: always comes from above; continues below only if a parent keeps it */}
      <line
        x1={laneX(c.lane)} y1={0} x2={laneX(c.lane)} y2={mid}
        stroke={laneColor(c.lane)} strokeWidth={2} opacity={0.55}
      />
      {continues && (
        <line
          x1={laneX(c.lane)} y1={mid} x2={laneX(c.lane)} y2={ROW_H}
          stroke={laneColor(c.lane)} strokeWidth={2} opacity={0.55}
        />
      )}
      {/* fan-out to parents living in other lanes (this is what a merge looks like) */}
      {c.parent_lanes
        .filter((l) => l !== c.lane)
        .map((l) => (
          <path
            key={`edge-${l}`}
            d={`M ${laneX(c.lane)} ${mid} C ${laneX(c.lane)} ${mid + 14}, ${laneX(l)} ${mid + 8}, ${laneX(l)} ${ROW_H}`}
            fill="none" stroke={laneColor(l)} strokeWidth={2} opacity={0.7}
          />
        ))}
      <circle
        cx={laneX(c.lane)} cy={mid} r={c.is_merge ? 5.5 : 4}
        fill={c.is_merge ? "#18181b" : laneColor(c.lane)}
        stroke={laneColor(c.lane)} strokeWidth={2}
      />
    </svg>
  );
}

export function History() {
  const toast = useToast();
  const [repos, setRepos] = useState<api.RepoRef[]>([]);
  const [repo, setRepo] = usePersistedState("history.repo", "");
  const [account, setAccount] = usePersistedState("history.account", ""); // profile id, "" = all
  const [onlyMine, setOnlyMine] = usePersistedState("history.onlyMine", false);
  const [branches, setBranches] = useState<api.BranchInfo[]>([]);
  const [rev, setRev] = usePersistedState("history.rev", "--all");
  const [page, setPage] = useState<api.HistoryPage | null>(null);
  const [offset, setOffset] = useState(0);
  const [search, setSearch] = useState("");
  const [appliedSearch, setAppliedSearch] = useState("");
  const [showRemote, setShowRemote] = usePersistedState("history.showRemote", false);
  const [mergeInfo, setMergeInfo] = useState<api.MergeInfo | null>(null);
  const [detail, setDetail] = useState<api.CommitDetail | null>(null);
  const [loadingRepos, setLoadingRepos] = useState(true);
  const [loadingBranches, setLoadingBranches] = useState(false);
  const [loadingPage, setLoadingPage] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sync, setSync] = useState<api.SyncStatus | null>(null);
  const [fetching, setFetching] = useState(false);
  const [autoFetch, setAutoFetch] = usePersistedState("history.autoFetch", false);
  const [reloadKey, setReloadKey] = useState(0);
  const pageSeq = useRef(0);

  const accounts = Array.from(
    new Map(
      repos.map((r) => [r.profile_id, { id: r.profile_id, name: r.profile_name, email: r.profile_email }])
    ).values()
  );
  const accountEmail = accounts.find((a) => a.id === account)?.email ?? "";
  const visibleRepos = account ? repos.filter((r) => r.profile_id === account) : repos;

  const isRange = rev.includes("..");

  const loadSync = useCallback(async (path: string, r: string) => {
    if (!path) return;
    try {
      setSync(await api.historySyncStatus(path, r.includes("..") ? "--all" : r));
    } catch {
      setSync(null);
    }
  }, []);

  /** git fetch — updates remote refs only; the working tree is never touched. */
  async function doFetch(path = repo, silent = false) {
    if (!path) return;
    setFetching(true);
    if (!silent) setError(null);
    try {
      const res = await api.historyFetch(path);
      await loadBranches(path);
      await loadSync(path, rev);
      setReloadKey((k) => k + 1); // re-read the commit page with fresh refs
      if (!silent) toast.success(res.message);
    } catch (e) {
      if (!silent) setError(String(e));
    } finally {
      setFetching(false);
    }
  }

  function showIncoming() {
    if (!sync?.incoming_rev) return;
    setRev(sync.incoming_rev);
    setOffset(0);
    setMergeInfo(null);
  }



  useEffect(() => {
    let cancelled = false;
    api.historyListRepos()
      .then((r) => {
        if (cancelled) return;
        setRepos(r);
        // Keep the remembered repo only if it still exists.
        setRepo((prev) =>
          prev && r.some((x) => x.path === prev) ? prev : r[0]?.path ?? ""
        );
      })
      .catch((e) => !cancelled && setError(String(e)))
      .finally(() => !cancelled && setLoadingRepos(false));
    return () => { cancelled = true; };
  }, []);

  const loadBranches = useCallback(async (path: string) => {
    if (!path) return;
    setLoadingBranches(true);
    try {
      setBranches(await api.historyBranches(path));
    } catch (e) {
      setError(String(e));
      setBranches([]);
    } finally {
      setLoadingBranches(false);
    }
  }, []);

  // Switching repo resets the view (a branch from another repo would query the
  // wrong thing) — but returning to the page must NOT reset, or the restored
  // selection would be wiped on mount. So only react to a genuine change.
  const prevRepo = useRef<string | null>(null);
  useEffect(() => {
    if (!repo) return;
    const switched = prevRepo.current !== null && prevRepo.current !== repo;
    prevRepo.current = repo;
    if (switched) {
      setRev("--all");
      setOffset(0);
      setMergeInfo(null);
      setSearch("");
      setAppliedSearch("");
    }
    loadBranches(repo);
  }, [repo, loadBranches]);

  // If the chosen account doesn't own the current repo, move to one it does —
  // otherwise the picker would show a repo the filter says shouldn't be there.
  useEffect(() => {
    if (!account) return;
    const owned = repos.filter((r) => r.profile_id === account);
    if (owned.length > 0 && !owned.some((r) => r.path === repo)) {
      setRepo(owned[0].path);
    }
  }, [account, repos, repo]);

  useEffect(() => {
    if (!repo) return;
    const seq = ++pageSeq.current;
    setLoadingPage(true);
    api.historyPage(
      repo,
      rev,
      offset,
      PAGE_SIZE,
      appliedSearch || null,
      onlyMine && accountEmail ? accountEmail : null
    )
      .then((p) => { if (pageSeq.current === seq) { setPage(p); setError(null); } })
      .catch((e) => { if (pageSeq.current === seq) { setError(String(e)); setPage(null); } })
      .finally(() => { if (pageSeq.current === seq) setLoadingPage(false); });
  }, [repo, rev, offset, appliedSearch, reloadKey, onlyMine, accountEmail]);

  useEffect(() => {
    loadSync(repo, rev);
  }, [repo, rev, loadSync]);

  // Opt-in: refresh remote refs whenever a repo is opened, so the counts and
  // "latest commit" are true without the user thinking about it.
  useEffect(() => {
    if (!repo || !autoFetch) return;
    doFetch(repo, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repo, autoFetch]);

  async function selectBranch(name: string) {
    setRev(name);
    setOffset(0);
    setMergeInfo(null);
    if (name !== "--all" && !name.includes("..")) {
      try {
        setMergeInfo(await api.historyBranchMerges(repo, name));
      } catch {
        setMergeInfo(null);
      }
    }
  }

  const visibleBranches = branches.filter((b) => b.is_remote === showRemote);
  const totalPages = page ? Math.max(1, Math.ceil(page.total / PAGE_SIZE)) : 1;
  const currentPage = Math.floor(offset / PAGE_SIZE) + 1;

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">History</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Browse branches, commits and how they merged — one repo at a time.
          </p>
        </div>
        <div className="flex items-center gap-2 min-w-0 flex-wrap">
          <div className="w-52 max-w-full">
            <Select
              value={account}
              onChange={(v) => { setAccount(v); setOffset(0); if (!v) setOnlyMine(false); }}
              placeholder="All accounts"
              optionIcon={<GitHubIcon size={14} />}
              options={[
                { value: "", label: "All accounts" },
                ...accounts.map((a) => ({ value: a.id, label: a.name })),
              ]}
            />
          </div>
          <div className="w-64 max-w-full">
            <Select
              value={repo}
              onChange={setRepo}
              placeholder={loadingRepos ? "Loading repos…" : "Pick a repository…"}
              optionIcon={<GitBranch size={14} />}
              options={visibleRepos.map((r) => ({
                value: r.path,
                label: account ? r.name : `${r.name}  ·  ${r.profile_name}`,
              }))}
            />
          </div>
          <Button
            variant="secondary"
            onClick={() => doFetch()}
            disabled={!repo || fetching}
            className="min-w-[8.25rem]" // fits "Fetching…" (≈125px), so the label swap never shifts Refresh
            title="git fetch — updates remote branches only, never your working tree"
          >
            <CloudDownload size={16} className={fetching ? "animate-pulse" : ""} />
            {fetching ? "Fetching…" : "Fetch"}
          </Button>
          <Button
            variant="secondary"
            onClick={() => { loadBranches(repo); setOffset(0); setReloadKey((k) => k + 1); }}
            disabled={!repo || loadingBranches}
          >
            <RefreshCw size={16} className={loadingBranches ? "animate-spin" : ""} />
            Refresh
          </Button>
        </div>
      </div>

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      {!loadingRepos && repos.length === 0 && (
        <Card className="text-center py-10">
          <GitBranch size={32} className="mx-auto text-zinc-600 mb-3" />
          <h3 className="text-lg font-medium text-zinc-300">No repositories found</h3>
          <p className="text-sm text-zinc-500 mt-1">
            Assign folders to a profile and GitSwitch will list the repos inside them here.
          </p>
        </Card>
      )}

      {repo && sync && (
        <div className="flex items-center gap-3 flex-wrap text-sm">
          {sync.behind > 0 ? (
            <div className="flex items-center gap-3 flex-wrap px-4 py-3 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-300 min-w-0">
              <CloudDownload size={16} className="shrink-0" />
              <span className="min-w-0">
                <strong>{sync.behind}</strong> commit{sync.behind === 1 ? "" : "s"} on{" "}
                <span className="font-mono">{sync.upstream}</span> you haven't pulled
                {sync.ahead > 0 && <> · you're also <strong>{sync.ahead}</strong> ahead</>}
              </span>
              {!isRange && (
                <Button size="sm" variant="secondary" onClick={showIncoming}>
                  Show incoming
                </Button>
              )}
            </div>
          ) : sync.upstream ? (
            <span className="inline-flex items-center gap-1.5 text-zinc-500">
              <CheckCircle2 size={14} className="text-emerald-400" />
              Up to date with <span className="font-mono">{sync.upstream}</span>
              {sync.ahead > 0 && <> · {sync.ahead} to push</>}
            </span>
          ) : (
            <span className="text-zinc-600">No upstream branch — nothing to compare against.</span>
          )}

          <span
            className={`text-xs ${sync.last_fetch_secs === null ? "text-amber-400/80" : "text-zinc-600"}`}
            title="Counts are only as fresh as the last fetch"
          >
            {fetchAgo(sync.last_fetch_secs)}
          </span>

          <label className="inline-flex items-center gap-2 text-xs text-zinc-500 cursor-pointer select-none ml-auto">
            <Checkbox
              checked={autoFetch}
              onChange={setAutoFetch}
            />
            Auto-fetch on open
          </label>
        </div>
      )}

      {repo && sync && (sync.local_tip || sync.remote_tip) && (
        <div className="grid sm:grid-cols-2 gap-3">
          <TipCard
            label="Your local branch"
            name={sync.branch}
            tip={sync.local_tip}
            accent="zinc"
            note={sync.ahead > 0 ? `${sync.ahead} commit${sync.ahead === 1 ? "" : "s"} not pushed` : undefined}
          />
          <TipCard
            label="Remote branch"
            name={sync.upstream ?? "no upstream"}
            tip={sync.remote_tip}
            accent={sync.behind > 0 ? "amber" : "emerald"}
            note={
              sync.behind > 0
                ? `${sync.behind} commit${sync.behind === 1 ? "" : "s"} ahead of you — not pulled`
                : "you have everything from here"
            }
          />
        </div>
      )}

      {isRange && (
        <div className="flex items-center gap-3 flex-wrap px-4 py-3 rounded-lg bg-zinc-800/60 border border-zinc-700/50 text-sm">
          <span className="text-zinc-300">
            Showing <strong>incoming</strong> commits — on the remote, not in your clone yet
          </span>
          <span className="font-mono text-xs text-zinc-500">{rev}</span>
          <Button size="sm" variant="ghost" onClick={() => selectBranch("--all")}>
            <X size={14} />
            Back to history
          </Button>
        </div>
      )}

      {repo && (
        <div className="flex flex-col lg:flex-row gap-6 min-w-0">
          {/* ---------------- branches ---------------- */}
          <div className="w-full lg:w-72 shrink-0 space-y-3">
            <Card>
              <div className="flex items-center gap-1 mb-3">
                {(["local", "remote"] as const).map((k) => {
                  const active = (k === "remote") === showRemote;
                  return (
                    <button
                      key={k}
                      onClick={() => setShowRemote(k === "remote")}
                      className={`px-2.5 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer ${
                        active ? "bg-zinc-800 text-emerald-400" : "text-zinc-500 hover:text-zinc-300"
                      }`}
                    >
                      {k === "local" ? "Local" : "Remote"}
                    </button>
                  );
                })}
                <span className="ml-auto text-xs text-zinc-600">
                  {visibleBranches.length}
                </span>
              </div>

              <button
                onClick={() => selectBranch("--all")}
                className={`w-full text-left px-3 py-2 rounded-lg mb-1.5 transition-colors cursor-pointer ${
                  rev === "--all"
                    ? "bg-emerald-500/10 border border-emerald-500/40"
                    : "bg-zinc-800/50 border border-zinc-700/40 hover:border-zinc-600"
                }`}
              >
                <span className="text-sm text-zinc-200">All branches</span>
              </button>

              {loadingBranches ? (
                <div className="flex items-center gap-2 py-4 text-xs text-zinc-500">
                  <Loader2 size={14} className="animate-spin text-emerald-400" />
                  Reading branches…
                </div>
              ) : (
                <div className="space-y-1.5 max-h-[26rem] overflow-y-auto pr-1">
                  {visibleBranches.map((b) => (
                    <button
                      key={b.name}
                      onClick={() => selectBranch(b.name)}
                      title={b.name}
                      className={`w-full text-left px-3 py-2 rounded-lg transition-colors cursor-pointer min-w-0 ${
                        rev === b.name
                          ? "bg-emerald-500/10 border border-emerald-500/40"
                          : "bg-zinc-800/50 border border-zinc-700/40 hover:border-zinc-600"
                      }`}
                    >
                      <div className="flex items-center gap-1.5 min-w-0">
                        <span className="text-sm text-zinc-200 font-mono truncate min-w-0 flex-1">
                          {b.name}
                        </span>
                        {b.is_current && <Badge variant="success">HEAD</Badge>}
                      </div>
                      <div className="flex items-center gap-2 mt-1 text-[11px] text-zinc-500">
                        {b.ahead > 0 && <span className="text-emerald-400">↑{b.ahead}</span>}
                        {b.behind > 0 && <span className="text-amber-400">↓{b.behind}</span>}
                        <span className="truncate min-w-0">{b.last_subject}</span>
                      </div>
                    </button>
                  ))}
                  {visibleBranches.length === 0 && (
                    <p className="text-xs text-zinc-600 py-2">
                      No {showRemote ? "remote" : "local"} branches.
                    </p>
                  )}
                </div>
              )}
            </Card>

            {mergeInfo && (
              <Card>
                <div className="flex items-center gap-2 mb-2">
                  <GitMerge size={15} className="text-emerald-400" />
                  <h3 className="text-sm font-semibold text-zinc-200">Merge status</h3>
                </div>
                {mergeInfo.merged_into.length > 0 ? (
                  <>
                    <p className="text-xs text-zinc-500 mb-2">
                      This branch is already contained in:
                    </p>
                    <div className="flex flex-wrap gap-1.5">
                      {mergeInfo.merged_into.map((b) => (
                        <Badge key={b} variant="success">{b}</Badge>
                      ))}
                    </div>
                  </>
                ) : (
                  <p className="text-xs text-zinc-500">
                    Not merged into any other branch yet.
                  </p>
                )}
                {mergeInfo.merges.length > 0 && (
                  <>
                    <p className="text-xs text-zinc-500 mt-3 mb-1.5">
                      Merge commits on this branch ({mergeInfo.merges.length}):
                    </p>
                    <div className="space-y-1 max-h-40 overflow-y-auto pr-1">
                      {mergeInfo.merges.map((m) => (
                        <p key={m.hash} className="text-[11px] text-zinc-400 truncate" title={m.subject}>
                          <span className="font-mono text-zinc-600">{m.short}</span> {m.subject}
                        </p>
                      ))}
                    </div>
                  </>
                )}
              </Card>
            )}
          </div>

          {/* ---------------- commits ---------------- */}
          <div className="flex-1 min-w-0 space-y-3">
            <Card>
              <form
                onSubmit={(e) => { e.preventDefault(); setOffset(0); setAppliedSearch(search); }}
                className="flex gap-2 mb-4"
              >
                <Input
                  placeholder="Search commit messages…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  containerClassName="flex-1 min-w-0"
                />
                <Button type="submit" variant="secondary">
                  <Search size={16} />
                  Search
                </Button>
                {appliedSearch && (
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => { setSearch(""); setAppliedSearch(""); setOffset(0); }}
                  >
                    <X size={16} />
                  </Button>
                )}
              </form>

              {account && (
                <label className="flex items-start gap-2.5 mb-3 cursor-pointer select-none">
                  <Checkbox
                    checked={onlyMine}
                    onChange={(v) => { setOnlyMine(v); setOffset(0); }}
                    className="mt-0.5"
                  />
                  <span className="min-w-0">
                    <span className="text-xs text-zinc-300">
                      Only commits authored by this account
                    </span>
                    <span className="block text-[11px] text-zinc-600 font-mono truncate">
                      {accountEmail}
                    </span>
                  </span>
                </label>
              )}

              <div className="flex items-center justify-between gap-3 flex-wrap mb-2">
                <p className="text-xs text-zinc-500">
                  {rev === "--all" ? "All branches" : rev}
                  {onlyMine && accountEmail && " · this account only"}
                  {page && (
                    <>
                      {" · "}
                      {page.total.toLocaleString()} commit{page.total === 1 ? "" : "s"}
                      {page.total > 0 && (
                        <>
                          {" · showing "}
                          {(offset + 1).toLocaleString()}–
                          {(offset + page.commits.length).toLocaleString()}
                        </>
                      )}
                    </>
                  )}
                </p>
                {loadingPage && (
                  <span className="inline-flex items-center gap-1.5 text-xs text-zinc-500">
                    <Loader2 size={12} className="animate-spin text-emerald-400" />
                    Loading…
                  </span>
                )}
              </div>

              {page && page.commits.length === 0 && !loadingPage && (
                <p className="text-sm text-zinc-500 py-6 text-center">
                  No commits{appliedSearch ? ` matching “${appliedSearch}”` : ""}
                  {onlyMine ? " authored by this account" : ""}.
                </p>
              )}

              <div className="divide-y divide-zinc-800/70">
                {page?.commits.map((c) => (
                  <button
                    key={c.hash}
                    onClick={async () => {
                      try {
                        setDetail(await api.historyCommitDetail(repo, c.hash));
                      } catch (e) {
                        setError(String(e));
                      }
                    }}
                    className="w-full flex items-center gap-3 text-left hover:bg-zinc-800/40 transition-colors cursor-pointer min-w-0 px-1"
                    style={{ height: ROW_H }}
                  >
                    <GraphCell c={c} maxLane={page.max_lane} />
                    <span className="font-mono text-xs text-zinc-500 shrink-0">{c.short}</span>
                    {c.is_merge && (
                      <GitMerge size={13} className="text-emerald-400 shrink-0" />
                    )}
                    <span className="text-sm text-zinc-200 truncate min-w-0 flex-1">
                      {c.subject}
                    </span>
                    <span className="hidden xl:flex items-center gap-1 shrink-0 max-w-[26%] overflow-hidden">
                      {c.refs.slice(0, 2).map((r) => (
                        <Badge key={r} variant="default">{r}</Badge>
                      ))}
                    </span>
                    <span className="hidden md:inline text-xs text-zinc-500 truncate min-w-0 max-w-[18%]">
                      {c.author_name}
                    </span>
                    <span className="hidden sm:inline text-xs text-zinc-600 shrink-0 tabular-nums">
                      {c.date.slice(0, 10)}
                    </span>
                  </button>
                ))}
              </div>

              {page && page.total > PAGE_SIZE && (
                <div className="flex items-center justify-between gap-3 flex-wrap pt-4 mt-2 border-t border-zinc-800">
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
                    disabled={offset === 0 || loadingPage}
                  >
                    <ChevronLeft size={14} />
                    Previous
                  </Button>
                  <span className="text-xs text-zinc-500">
                    Page {currentPage.toLocaleString()} of {totalPages.toLocaleString()}
                  </span>
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => setOffset(offset + PAGE_SIZE)}
                    disabled={!page.has_more || loadingPage}
                  >
                    Next
                    <ChevronRight size={14} />
                  </Button>
                </div>
              )}
            </Card>
          </div>
        </div>
      )}

      {detail && <CommitDetailPanel detail={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}

function TipCard({
  label,
  name,
  tip,
  note,
  accent,
}: {
  label: string;
  name: string;
  tip: api.CommitRef | null;
  note?: string;
  accent: "zinc" | "amber" | "emerald";
}) {
  const ring =
    accent === "amber"
      ? "border-amber-500/30"
      : accent === "emerald"
      ? "border-emerald-500/25"
      : "border-zinc-700/50";
  const noteColor =
    accent === "amber" ? "text-amber-400" : accent === "emerald" ? "text-emerald-400/80" : "text-zinc-500";
  return (
    <div className={`rounded-xl border ${ring} bg-zinc-900/50 px-4 py-3 min-w-0`}>
      <div className="flex items-center gap-2 min-w-0">
        <span className="text-[11px] uppercase tracking-wider text-zinc-500 shrink-0">{label}</span>
        <span className="font-mono text-xs text-zinc-300 truncate min-w-0">{name}</span>
      </div>
      {tip ? (
        <>
          <p className="text-sm text-zinc-200 truncate mt-1.5" title={tip.subject}>
            <span className="font-mono text-zinc-500 mr-2">{tip.short}</span>
            {tip.subject}
          </p>
          <p className="text-[11px] text-zinc-600 truncate mt-0.5">
            {tip.author} · {tip.date.replace("T", " ").slice(0, 16)}
          </p>
        </>
      ) : (
        <p className="text-sm text-zinc-600 mt-1.5">—</p>
      )}
      {note && <p className={`text-[11px] mt-1 ${noteColor}`}>{note}</p>}
    </div>
  );
}

function CommitDetailPanel({
  detail,
  onClose,
}: {
  detail: api.CommitDetail;
  onClose: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />
      <div className="relative w-full max-w-3xl max-h-[85vh] overflow-y-auto rounded-xl border border-zinc-700/50 bg-zinc-900 p-6 shadow-2xl">
        <div className="flex items-start justify-between gap-3 mb-4">
          <div className="min-w-0">
            <h2 className="text-lg font-semibold text-zinc-100 break-words">
              {detail.subject}
            </h2>
            <p className="text-xs text-zinc-500 font-mono mt-1 break-all">{detail.hash}</p>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
          >
            <X size={18} />
          </button>
        </div>

        <div className="flex flex-wrap gap-2 mb-4">
          {detail.parents.length > 1 && <Badge variant="success">merge commit</Badge>}
          {detail.refs.map((r) => (
            <Badge key={r} variant="default">{r}</Badge>
          ))}
          {detail.signature && (
            <span className="inline-flex items-center gap-1 text-xs text-emerald-400">
              <ShieldCheck size={13} />
              {detail.signature}
            </span>
          )}
        </div>

        <div className="grid sm:grid-cols-2 gap-3 text-xs mb-4">
          <div className="px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/40 min-w-0">
            <p className="text-zinc-500">Author</p>
            <p className="text-zinc-200 truncate">{detail.author_name}</p>
            <p className="text-zinc-500 font-mono truncate">{detail.author_email}</p>
            <p className="text-zinc-600 mt-1">{detail.author_date.replace("T", " ").slice(0, 19)}</p>
          </div>
          <div className="px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/40 min-w-0">
            <p className="text-zinc-500">
              Parent{detail.parents.length === 1 ? "" : "s"}
              {detail.parents.length > 1 && " — this is where two branches joined"}
            </p>
            {detail.parents.length === 0 && (
              <p className="text-zinc-400">none (root commit)</p>
            )}
            {detail.parents.map((p) => (
              <p key={p} className="text-zinc-300 font-mono truncate">{p.slice(0, 12)}</p>
            ))}
          </div>
        </div>

        {detail.body && (
          <pre className="p-3 rounded-lg bg-zinc-800/50 border border-zinc-700/40 text-xs text-zinc-300 whitespace-pre-wrap break-words mb-4 max-h-52 overflow-y-auto">
            {detail.body}
          </pre>
        )}

        <div className="flex items-center gap-2 mb-2">
          <FileText size={14} className="text-zinc-500" />
          <h3 className="text-sm font-medium text-zinc-300">
            {detail.files.length} file{detail.files.length === 1 ? "" : "s"} changed
          </h3>
        </div>
        <div className="space-y-1 max-h-64 overflow-y-auto pr-1">
          {detail.files.map((f) => (
            <div
              key={f.path}
              className="flex items-center gap-3 px-3 py-1.5 rounded-lg bg-zinc-800/40 border border-zinc-700/30 text-xs min-w-0"
            >
              <span className="font-mono text-zinc-300 truncate min-w-0 flex-1" title={f.path}>
                {f.path}
              </span>
              <span className="text-emerald-400 shrink-0 tabular-nums">+{f.added}</span>
              <span className="text-red-400 shrink-0 tabular-nums">−{f.removed}</span>
            </div>
          ))}
          {detail.files.length === 0 && (
            <p className="text-xs text-zinc-600">No file changes (empty or merge commit).</p>
          )}
        </div>
      </div>
    </div>
  );
}
