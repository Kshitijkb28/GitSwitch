import { useState, useEffect, useRef, useCallback, type SubmitEvent } from "react";
import {
  GitMerge,
  Loader2,
  ChevronLeft,
  ChevronRight,
  Search,
  CloudDownload,
  CheckCircle2,
  Crosshair,
  X,
  UserRound,
} from "lucide-react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Card } from "../Card";
import { Badge } from "../Badge";
import { Input } from "../Input";
import { Select } from "../Select";
import { useToast } from "../Toast";
import { ResultCard } from "../changes/ResultCard";
import { CommitDetailPanel } from "../history/CommitDetailPanel";
import { usePersistedState } from "../../lib/persist";
import { useGitJob } from "../../lib/gitJobs";
import * as api from "../../lib/api";
import { formatWhen } from "../../lib/time";

/** Refusals the detail panel answers inline (a note, a re-asked question) — it stays open for these. */
const PANEL_HANDLES = new Set(["would-drop-pushed", "merge-commit-needs-mainline"]);

const PAGE_SIZE = 50;
const BRANCH_PAGE = 50;
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

interface Props {
  /** The repository — or the submodule in scope — whose history this is. */
  repo: string;
  /** The account filter's profile id ("" = all) and that profile's email, for "only mine". */
  account: string;
  accountEmail: string;
  /** The accounts have been read: until then `accountEmail` is empty for a reason that will pass. */
  accountsReady: boolean;
  visible: boolean;
  /** Bumped by the page when the repository moved (a fetch, a pull, any operation). */
  reloadKey: number;
  onOpenChanges: () => void;
  /** This tab moved the repository (a fetch, a detach, a reset…): the strip must re-read it. */
  onRepoChanged: () => void;
  /** The repository has submodules: pointer-only commits can be hidden. */
  hasSubmodules: boolean;
}

/** The most `--author` patterns one history read is given (the Windows command line has a limit). */
const MAX_AUTHOR_FILTER = 200;
/** How many people the backend lists at most: a list that long may be cut short. */
const AUTHORS_LISTED = 300;

/**
 * What a person is filtered by. An address is matched whole by the backend
 * (it wraps it in its angle brackets). A commit with no address, or with one
 * that is not an address (`jdoe <jdoe>`, an import), is matched by its whole
 * identity, anchored at both ends — a bare name would also mean everyone
 * whose name or address merely contains it.
 */
function personKey(a: api.AuthorCount): string {
  return a.email.includes("@") ? a.email : `${a.name} <${a.email}>`;
}

/** A bot or a CI identity, by the names and addresses they use. */
function isBot(a: api.AuthorCount): boolean {
  return /\[bot\]/i.test(a.name) || /^(github|github actions)$/i.test(a.name.trim()) || /\[bot\]|noreply@github\.com|^actions@|^ci@|dependabot|renovate|github-actions/i.test(a.email);
}

/**
 * Branches, the commit graph and the detail panel: what used to be the whole
 * History page, minus its picker, Fetch and Check GitHub, which now sit above
 * the tabs.
 */
export function HistoryTab({ repo, account, accountEmail, accountsReady, visible, reloadKey: outerReload, onOpenChanges, onRepoChanged, hasSubmodules }: Props) {
  const toast = useToast();
  const [onlyMine, setOnlyMine] = usePersistedState("history.onlyMine", false);
  // The people filter: an author email, "" for everyone. It beats "only this
  // account" when both are set, since it is the more specific choice.
  const [author, setAuthor] = usePersistedState("history.author", "");
  // CI bumps fill a superproject's history; this hides commits that move
  // nothing but submodule pointers.
  const [hidePointerOnly, setHidePointerOnly] = usePersistedState("history.hidePointerOnly", false);
  // What a push would send: the commits no remote-tracking branch has.
  const [onlyUnpushed, setOnlyUnpushed] = usePersistedState("history.onlyUnpushed", false);
  const [authors, setAuthors] = useState<api.AuthorCount[]>([]);
  const authorsSeq = useRef(0);
  const [branches, setBranches] = useState<api.BranchInfo[]>([]);
  const [rev, setRev] = usePersistedState("history.rev", "--all");
  const [page, setPage] = useState<api.HistoryPage | null>(null);
  const [offset, setOffset] = useState(0);
  const [search, setSearch] = useState("");
  const [appliedSearch, setAppliedSearch] = useState("");
  const [showRemote, setShowRemote] = usePersistedState("history.showRemote", false);
  const [mergeInfo, setMergeInfo] = useState<api.MergeInfo | null>(null);
  const [detail, setDetail] = useState<api.CommitDetail | null>(null);
  /** What "Go to commit" resolved — carried into the panel so the chooser needn't ask again. */
  const [target, setTarget] = useState<api.CommitTarget | null>(null);
  const [head, setHead] = useState<api.HeadInfo | null>(null);
  const [gotoText, setGotoText] = useState("");
  const [gotoError, setGotoError] = useState<string | null>(null);
  const [gotoBusy, setGotoBusy] = useState(false);
  /** The outcome of the last action taken from the detail panel. */
  const [result, setResult] = useState<api.OpResult | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const job = useGitJob(repo);
  const [loadingBranches, setLoadingBranches] = useState(false);
  const [loadingPage, setLoadingPage] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sync, setSync] = useState<api.SyncStatus | null>(null);
  const [, setFetching] = useState(false);
  const [autoFetch, setAutoFetch] = usePersistedState("history.autoFetch", false);
  const [reloadKey, setReloadKey] = useState(0);
  const pageSeq = useRef(0);

  const isRange = rev.includes("..");

  const loadSync = useCallback(async (path: string, r: string) => {
    if (!path) return;
    try {
      setSync(await api.historySyncStatus(path, r.includes("..") ? "--all" : r));
    } catch {
      setSync(null);
    }
  }, []);

  /** The Merge status card: which branches contain `r`. Only for a single branch, and re-read after anything that moved one. */
  const loadMergeInfo = useCallback(async (path: string, r: string) => {
    if (!path || r === "--all" || r.includes("..")) {
      setMergeInfo(null);
      return;
    }
    try {
      setMergeInfo(await api.historyBranchMerges(path, r));
    } catch {
      setMergeInfo(null);
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
      await loadMergeInfo(path, rev);
      setReloadKey((k) => k + 1); // re-read the commit page with fresh refs
      onRepoChanged();
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



  // The page above says when the repository moved (a fetch, a pull, any
  // operation on the Changes tab): re-read everything that could have changed.
  useEffect(() => {
    if (!outerReload || !repo) return;
    loadBranches(repo);
    loadSync(repo, rev);
    loadMergeInfo(repo, rev);
    setReloadKey((k) => k + 1);
  }, [outerReload]);

  const loadBranches = useCallback(async (path: string) => {
    if (!path) return;
    setLoadingBranches(true);
    // Which branch HEAD is on (or that it is detached): the branch list alone
    // can't tell a detached HEAD from an unborn one. Three git reads, not the
    // Changes page's whole status.
    api.historyHead(path).then(setHead).catch(() => setHead(null));
    try {
      setBranches(await api.historyBranches(path));
    } catch (e) {
      setError(String(e));
      setBranches([]);
    } finally {
      setLoadingBranches(false);
    }
  }, []);

  const currentBranch =
    head?.branch ?? branches.find((b) => b.is_current && !b.is_remote)?.name ?? null;
  const detached = head ? head.detached : branches.length > 0 && !currentBranch;

  async function openCommit(hash: string, resolved: api.CommitTarget | null = null) {
    try {
      const d = await api.historyCommitDetail(repo, hash);
      setTarget(resolved);
      setDetail(d);
    } catch (e) {
      setError(String(e));
    }
  }

  function closePanel() {
    setDetail(null);
    setTarget(null);
  }

  /** "Go to commit": any id, short id or ref the user typed. */
  async function gotoCommit(e: SubmitEvent<HTMLFormElement>) {
    e.preventDefault();
    const text = gotoText.trim();
    if (!text || !repo) return;
    setGotoBusy(true);
    setGotoError(null);
    try {
      const t = await api.historyResolve(repo, text);
      if (!t) {
        setGotoError(`No commit matches "${text}" in this repository.`);
        return;
      }
      await openCommit(t.oid, t);
    } catch (err) {
      setGotoError(String(err));
    } finally {
      setGotoBusy(false);
    }
  }

  const resolveCommit = useCallback(
    async (hash: string) => {
      const t = await api.historyResolve(repo, hash);
      setTarget(t);
      return t;
    },
    [repo]
  );

  /**
   * One git operation from the detail panel. The panel closes and the outcome
   * shows as a card under the header — except for the refusals the panel
   * answers itself, which keep it open. Everything is re-read afterwards: a
   * reset moves a branch, a revert adds a commit, a detach changes HEAD.
   */
  async function onAction(key: string, fn: () => Promise<api.OpResult>): Promise<api.OpResult | undefined> {
    if (busyKey) return undefined;
    if (job?.running) {
      toast.error("Wait for the running operation");
      return undefined;
    }
    setBusyKey(key);
    setError(null);
    try {
      const r = await fn();
      setResult(r);
      if (r.ok) toast.success(r.headline);
      else toast.error(r.refusal?.message ?? r.advice?.headline ?? r.headline);
      if (!(r.refusal && PANEL_HANDLES.has(r.refusal.code))) closePanel();
      await loadBranches(repo);
      await loadSync(repo, rev);
      // A reset or a new branch changes which branches contain this one.
      await loadMergeInfo(repo, rev);
      setReloadKey((k) => k + 1);
      onRepoChanged();
      return r;
    } catch (e) {
      setError(String(e));
      toast.error(String(e));
      return undefined;
    } finally {
      setBusyKey(null);
    }
  }

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
      setGotoText("");
      setGotoError(null);
      setResult(null);
      setDetail(null);
      setTarget(null);
      setBranchQuery("");
    }
    loadBranches(repo);
  }, [repo, loadBranches]);

  // The people filter resolves to a list of emails: one person, "me", or
  // everyone who is not a bot. Null means everyone.
  const people = authors.filter((a) => !isBot(a));
  const bots = authors.length - people.length;
  const mine = accountEmail ? authors.find((a) => a.email.toLowerCase() === accountEmail.toLowerCase()) : undefined;
  // How "me" is asked for: an address as it is (the backend matches it whole);
  // an account "address" without an @ (an import's `sam <sam>`) by the whole
  // identity when that person is in the list, else by the address in its
  // brackets — a bare word would also mean everyone whose name or address
  // contains it.
  const meFilter: string[] | null = !accountEmail ? null : accountEmail.includes("@") ? [accountEmail] : mine ? [personKey(mine)] : [`<${accountEmail}>`];
  // "People only" is the list of everyone who is not a bot, so it is offered
  // only while there is a bot to leave out and the list is one git can be
  // handed whole — cut short, it would hide people instead of bots.
  const peopleOffered = bots > 0 && people.length > 0 && people.length <= MAX_AUTHOR_FILTER && authors.length < AUTHORS_LISTED;
  const effectiveAuthors: string[] | null =
    author === "me"
      ? meFilter
      : author === "people"
        ? peopleOffered
          ? people.map(personKey)
          : null
        : author
          ? [author]
          : onlyMine
            ? meFilter
            : null;
  const authorsKey = JSON.stringify(effectiveAuthors);
  // "Just me" before the accounts are read would be everyone's commits under
  // that label; the page waits (and says it is loading) until they are.
  const awaitingMe = !accountsReady && (author === "me" || (!author && onlyMine && !!account));
  useEffect(() => {
    if (!repo) return;
    if (awaitingMe) {
      setLoadingPage(true);
      return;
    }
    const seq = ++pageSeq.current;
    setLoadingPage(true);
    api.historyPage(repo, rev, offset, PAGE_SIZE, appliedSearch || null, effectiveAuthors, hasSubmodules && hidePointerOnly, onlyUnpushed)
      .then((p) => {
        if (pageSeq.current !== seq) return;
        // The history shrank under a later page (a reset, a push with "only
        // not pushed" on): go back to the last page that exists. Terminates —
        // the offset only ever drops, towards a real page or the first.
        if (offset > 0 && p.commits.length === 0) {
          setOffset(p.total > 0 ? Math.floor((p.total - 1) / PAGE_SIZE) * PAGE_SIZE : 0);
          return;
        }
        setPage(p);
        setError(null);
      })
      .catch((e) => { if (pageSeq.current === seq) { setError(String(e)); setPage(null); } })
      .finally(() => { if (pageSeq.current === seq) setLoadingPage(false); });
  }, [repo, rev, offset, appliedSearch, reloadKey, authorsKey, hidePointerOnly, hasSubmodules, awaitingMe, onlyUnpushed]);

  // Who commits here — read once per repository and revision, never polled.
  useEffect(() => {
    if (!repo) { setAuthors([]); return; }
    const seq = ++authorsSeq.current;
    api.historyAuthors(repo, rev)
      .then((list) => { if (authorsSeq.current === seq) setAuthors(list); })
      .catch(() => { if (authorsSeq.current === seq) setAuthors([]); });
  }, [repo, rev, reloadKey]);
  // A person who is not in this revision's list is no filter at all — and so
  // is a preset that does not apply here (kept from another repository).
  const iCommitHere = !!mine;
  useEffect(() => {
    if (!author || !authors.length) return;
    // "Just me" cannot be judged before the accounts are read: the people
    // usually arrive first, and the preset would be thrown away on every visit.
    if (author === "me" && !accountsReady) return;
    const stale = author === "me" ? !iCommitHere : author === "people" ? !peopleOffered : !authors.some((a) => personKey(a) === author);
    if (stale) setAuthor("");
  }, [authors, author, iCommitHere, peopleOffered, setAuthor, accountsReady]);
  const authorName = author === "me" ? "me" : author === "people" ? "people only" : (authors.find((a) => personKey(a) === author)?.name ?? author);

  useEffect(() => {
    loadSync(repo, rev);
  }, [repo, rev, loadSync]);

  // Opt-in: refresh remote refs whenever a repo is opened, so the counts and
  // "latest commit" are true without the user thinking about it.
  useEffect(() => {
    if (!repo || !autoFetch) return;
    doFetch(repo, true);
  }, [repo, autoFetch]);

  async function selectBranch(name: string) {
    setRev(name);
    setOffset(0);
    setMergeInfo(null);
    await loadMergeInfo(repo, name);
  }

  const visibleBranches = branches.filter((b) => b.is_remote === showRemote);
  // A repository can have hundreds of branches: a filter, and the list grows
  // fifty at a time as it is scrolled — as every long list in the app does.
  const [branchQuery, setBranchQuery] = useState("");
  const [branchShown, setBranchShown] = useState(BRANCH_PAGE);
  const q = branchQuery.trim().toLowerCase();
  const filteredBranches = q ? visibleBranches.filter((b) => b.name.toLowerCase().includes(q)) : visibleBranches;
  useEffect(() => setBranchShown(BRANCH_PAGE), [branchQuery, showRemote, repo]);
  const totalPages = page ? Math.max(1, Math.ceil(page.total / PAGE_SIZE)) : 1;
  const currentPage = Math.floor(offset / PAGE_SIZE) + 1;

  return (
    <div className={`space-y-6 ${visible ? "" : "hidden"}`} role="tabpanel" aria-label="History">
      {gotoError && (
        <p className="text-sm text-red-400 break-words">{gotoError}</p>
      )}

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      {result && (
        <ResultCard
          result={result}
          busy={busyKey !== null}
          onDismiss={() => setResult(null)}
          extraAction={{
            label: "Open in Changes",
            onClick: onOpenChanges,
            primary: !result.ok || (result.status?.conflicted_count ?? 0) > 0,
          }}
        />
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
                  {q ? `${filteredBranches.length} of ${visibleBranches.length}` : visibleBranches.length}
                </span>
              </div>
              {visibleBranches.length > 3 && (
                <Input
                  aria-label="Filter branches"
                  placeholder="Filter branches"
                  value={branchQuery}
                  onChange={(e) => setBranchQuery(e.target.value)}
                  containerClassName="mb-2"
                  className="py-1.5 text-sm"
                />
              )}

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
                <div
                  className="space-y-1.5 max-h-[26rem] overflow-y-auto pr-1"
                  onScroll={(e) => {
                    const el = e.currentTarget;
                    if (el.scrollHeight - el.scrollTop - el.clientHeight < 64) {
                      setBranchShown((n) => (n < filteredBranches.length ? n + BRANCH_PAGE : n));
                    }
                  }}
                >
                  {filteredBranches.slice(0, branchShown).map((b) => (
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
                  {visibleBranches.length > 0 && filteredBranches.length === 0 && (
                    <p className="text-xs text-zinc-600 py-2">No branch matches “{branchQuery.trim()}”.</p>
                  )}
                  {filteredBranches.length > branchShown && (
                    <p className="text-xs text-zinc-500 py-2">
                      Showing {branchShown} of {filteredBranches.length}. Keep scrolling, or type to narrow it down.
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
              <div className="flex flex-wrap items-center gap-2 mb-4">
              <form onSubmit={gotoCommit} className="flex items-center gap-2 min-w-0">
                <Input
                  aria-label="Go to commit"
                  placeholder="commit id, short id or ref"
                  value={gotoText}
                  onChange={(e) => { setGotoText(e.target.value); if (gotoError) setGotoError(null); }}
                  disabled={!repo}
                  spellCheck={false}
                  containerClassName="w-56 max-w-full"
                  className="font-mono text-sm"
                />
                <Button type="submit" variant="secondary" disabled={!repo || gotoBusy || !gotoText.trim()} title="Open a commit by its id or a ref like main~3">
                  {gotoBusy ? <Loader2 size={16} className="animate-spin" /> : <Crosshair size={16} />}
                  Go
                </Button>
              </form>
              <form
                onSubmit={(e) => { e.preventDefault(); setOffset(0); setAppliedSearch(search); }}
                className="flex gap-2 flex-1 min-w-[16rem]"
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
              </div>

              {authors.length > 1 && (
                <div className="flex items-center gap-2 mb-3 flex-wrap">
                  <span className="text-xs text-zinc-500">People</span>
                  <div className="w-72 max-w-full">
                    <Select
                      value={author}
                      onChange={(v) => { setAuthor(v); setOffset(0); }}
                      searchable
                      searchLabel="Search people"
                      placeholder="Everyone"
                      optionIcon={<UserRound size={14} />}
                      options={[
                        { value: "", label: `Everyone · ${authors.reduce((n, a) => n + a.commits, 0).toLocaleString()}` },
                        ...(mine ? [{ value: "me", label: `Just me · ${mine.commits.toLocaleString()}` }] : []),
                        ...(peopleOffered
                          ? [{ value: "people", label: `People only (hide bots and CI) · ${people.reduce((n, a) => n + a.commits, 0).toLocaleString()}` }]
                          : []),
                        ...authors.map((a) => ({
                          value: personKey(a),
                          label: `${a.name}${isBot(a) ? " (bot)" : ""} · ${a.commits.toLocaleString()}`,
                        })),
                      ]}
                    />
                  </div>
                  {author && author !== "me" && author !== "people" && (
                    <span className="text-[11px] text-zinc-600 font-mono truncate max-w-[16rem]">{author}</span>
                  )}
                </div>
              )}

              {hasSubmodules && (
                <label className="flex items-start gap-2.5 mb-3 cursor-pointer select-none" title="A commit that changes nothing but submodule pointers — the bumps a CI makes — is left out of the list and the count">
                  <Checkbox
                    checked={hidePointerOnly}
                    onChange={(v) => { setHidePointerOnly(v); setOffset(0); }}
                    className="mt-0.5"
                    aria-label="Hide pointer-only commits"
                  />
                  <span className="text-xs text-zinc-300">
                    Hide pointer-only commits
                    <span className="text-zinc-500"> — the ones that only move a submodule pointer</span>
                  </span>
                </label>
              )}

              <label className="flex items-start gap-2.5 mb-3 cursor-pointer select-none" title="Commits no remote-tracking branch has yet — any remote counts">
                <Checkbox
                  checked={onlyUnpushed}
                  onChange={(v) => { setOnlyUnpushed(v); setOffset(0); }}
                  className="mt-0.5"
                  aria-label="Only commits not pushed yet"
                />
                <span className="text-xs text-zinc-300">
                  Only commits not pushed yet
                  <span className="text-zinc-500"> — the ones no remote-tracking branch has</span>
                </span>
              </label>

              {page && !page.remote_refs && (
                <p className="text-xs text-amber-300/90 mb-2 break-words">
                  No remote-tracking branch here: nothing is known to be pushed, so the rows carry no mark.
                </p>
              )}
              {page?.marks_error && (
                <p className="text-xs text-amber-300/90 mb-2 break-words">
                  Could not tell which commits a remote-tracking branch has, so the rows carry no mark: {page.marks_error}
                </p>
              )}

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
                  {author ? ` · by ${authorName}` : onlyMine && accountEmail ? " · this account only" : ""}
                  {onlyUnpushed ? " · not pushed yet" : ""}
                  {page && (
                    <>
                      {" · "}
                      {page.total.toLocaleString()} commit{page.total === 1 ? "" : "s"}
                      {page.commits.length > 0 && (
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
                  {author ? ` by ${authorName}` : onlyMine ? " authored by this account" : ""}
                  {onlyUnpushed ? " left that no remote-tracking branch has" : ""}.
                </p>
              )}

              <div className="divide-y divide-zinc-800/70">
                {page?.commits.map((c) => (
                  <button
                    key={c.hash}
                    onClick={() => openCommit(c.hash)}
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
                    {c.only_submodules && (
                      <span
                        className="hidden sm:inline text-[10px] px-1.5 py-0.5 rounded bg-zinc-800 text-zinc-500 shrink-0 truncate max-w-[12rem]"
                        title={`Changes nothing but the pointer of ${c.touches_submodules.join(", ")}`}
                      >
                        submodule bump: {c.touches_submodules.join(", ")}
                      </span>
                    )}
                    {c.unpushed && page?.remote_refs && (
                      <span
                        className="text-[10px] px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 shrink-0"
                        title="No remote-tracking branch has this commit: as far as this checkout knows, it exists only here"
                      >
                        not pushed
                      </span>
                    )}
                    <span className="hidden xl:flex items-center gap-1 shrink-0 max-w-[26%] overflow-hidden">
                      {c.refs.slice(0, 2).map((r) => (
                        <Badge key={r} variant="default">{r}</Badge>
                      ))}
                    </span>
                    <span className="hidden md:inline text-xs text-zinc-500 truncate min-w-0 max-w-[18%]">
                      {c.author_name}
                    </span>
                    <span
                      className="hidden sm:inline text-xs text-zinc-600 shrink-0 tabular-nums"
                      title={`${formatWhen(c.author_date || c.date).title}${c.author_date && c.author_date !== c.date ? ` · committed ${c.date}` : ""}`}
                    >
                      {formatWhen(c.author_date || c.date).text}
                    </span>
                  </button>
                ))}
              </div>

              {page && (offset > 0 || page.total > PAGE_SIZE) && (
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

      {detail && (
        <CommitDetailPanel
          key={detail.hash}
          detail={detail}
          target={target && target.oid === detail.hash ? target : null}
          repoPath={repo}
          currentBranch={currentBranch}
          detached={detached}
          busy={busyKey !== null}
          onClose={closePanel}
          onAction={onAction}
          onResolve={resolveCommit}
        />
      )}
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
            {tip.author} · <span title={formatWhen(tip.date).title}>{formatWhen(tip.date).text}</span>
          </p>
        </>
      ) : (
        <p className="text-sm text-zinc-600 mt-1.5">—</p>
      )}
      {note && <p className={`text-[11px] mt-1 ${noteColor}`}>{note}</p>}
    </div>
  );
}

