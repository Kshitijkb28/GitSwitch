import { useEffect, useRef, useState } from "react";
import { Archive, ExternalLink, GitPullRequest, Loader2, Lock, Search as SearchIcon } from "lucide-react";
import * as api from "../lib/api";
import { usePersistedState } from "../lib/persist";
import { Badge } from "../components/Badge";
import { Button } from "../components/Button";
import { GitHubIcon } from "../components/GitHubIcon";
import { Input } from "../components/Input";
import { RefreshButton } from "../components/RefreshButton";
import { Select } from "../components/Select";
import { PullList } from "../components/github/PullList";
import { PullPanel } from "../components/github/PullPanel";
import { Segmented, cleanError, openOnGitHub } from "../components/github/shared";

type Whose = "everyone" | api.PullScope;
type RepoFilter = "open" | "review" | "all";

const WHOSE: { value: Whose; label: string }[] = [
  { value: "everyone", label: "Everyone's" },
  { value: "created", label: "Created by me" },
  { value: "review_requested", label: "Review requested" },
  { value: "assigned", label: "Assigned" },
  { value: "involves", label: "Involving me" },
];
const REPO_FILTERS: { value: RepoFilter; label: string }[] = [
  { value: "open", label: "With open PRs" },
  { value: "review", label: "To review" },
  { value: "all", label: "All" },
];
const STATES: api.PullState[] = ["open", "merged", "closed", "all"];

/** A remembered value only when it is one of today's choices (an older build may have saved another). */
const known = <T extends string>(v: string, choices: readonly T[], fallback: T): T => ((choices as readonly string[]).includes(v) ? (v as T) : fallback);

/**
 * Each account's repositories and their counts, kept while the app runs:
 * coming back to the page shows them at once; Refresh reads them again. A
 * second ask while one is on its way joins it.
 */
const repoCache = new Map<string, api.PullRepos>();
const repoAsks = new Map<string, Promise<api.PullRepos>>();
function readRepos(account: string, again: boolean): Promise<api.PullRepos> {
  if (!again) {
    const kept = repoCache.get(account);
    if (kept) return Promise.resolve(kept);
    const running = repoAsks.get(account);
    if (running) return running;
  }
  const ask = api
    .githubPullRepos(account)
    .then((r) => {
      repoCache.set(account, r);
      return r;
    })
    .finally(() => {
      if (repoAsks.get(account) === ask) repoAsks.delete(account);
    });
  repoAsks.set(account, ask);
  return ask;
}

const hasOpen = (r: api.PullRepo) => (r.open ?? 0) > 0 || (r.to_review ?? 0) > 0 || (r.mine ?? 0) > 0;

/**
 * Pull requests, repository by repository: the account's repositories with
 * how many pull requests are open, asking it for a review, or its own; the
 * picked repository's pull requests beside them — everyone's, or created by,
 * asking a review of, assigned to or involving the account — open, merged,
 * closed or all. A row opens it to review and merge, as that account.
 */
export function Pulls() {
  const [accounts, setAccounts] = useState<api.RepoAccount[] | null>(null);
  const [accountsError, setAccountsError] = useState<string | null>(null);
  const [account, setAccount] = usePersistedState("pulls.account", "");
  const [repos, setRepos] = useState<api.PullRepos | null>(null);
  const [reposError, setReposError] = useState<string | null>(null);
  const [loadingRepos, setLoadingRepos] = useState(false);
  const [picked, setPicked] = usePersistedState<Record<string, string>>("pulls.repoByAccount", {});
  const [savedFilter, setRepoFilter] = usePersistedState<string>("pulls.repoFilter", "open");
  const [savedWhose, setWhose] = usePersistedState<string>("pulls.whose", "everyone");
  const [savedState, setState] = usePersistedState<string>("pulls.state", "open");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<api.PullItem | null>(null);
  const [reload, setReload] = useState(0);
  /** The repository list asked for last; an older answer is dropped. */
  const asked = useRef(0);

  const repoFilter = known(savedFilter, ["open", "review", "all"] as const, "open");
  const whose = known(savedWhose, ["everyone", "created", "review_requested", "assigned", "involves"] as const, "everyone");
  const state = known(savedState, STATES, "open");

  const readAccounts = () =>
    api
      .repoAccounts()
      .then((list) => {
        setAccounts(list);
        setAccountsError(null);
        if (list.length && !list.some((a) => a.key === account)) setAccount(list[0].key);
      })
      .catch((e) => {
        setAccounts([]);
        setAccountsError(cleanError(e));
      });

  async function loadRepos(again: boolean) {
    if (!account) return;
    const id = ++asked.current;
    setLoadingRepos(true);
    setReposError(null);
    try {
      const r = await readRepos(account, again);
      if (id === asked.current) setRepos(r);
    } catch (e) {
      if (id === asked.current) {
        setRepos(null);
        setReposError(cleanError(e));
      }
    } finally {
      if (id === asked.current) setLoadingRepos(false);
    }
  }

  useEffect(() => {
    void readAccounts();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const ready = !!accounts && accounts.some((a) => a.key === account);
  useEffect(() => {
    setOpen(null);
    setRepos(repoCache.get(account) ?? null);
    if (ready) void loadRepos(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account, ready]);

  function refresh() {
    void readAccounts();
    void loadRepos(true);
    setReload((n) => n + 1);
  }

  const login = repos?.login || (account.split(":")[1] ?? "");
  const all = repos?.repos ?? [];
  const q = query.trim().toLowerCase();
  const visible = all
    .filter((r) => repoFilter === "all" || (repoFilter === "open" ? hasOpen(r) : (r.to_review ?? 0) > 0))
    .filter((r) => !q || r.repo.toLowerCase().includes(q));
  const remembered = picked[account];
  const current = visible.find((r) => r.repo === remembered) ?? visible[0] ?? null;
  const [owner, name] = current ? current.repo.split("/") : ["", ""];
  const counts = { open: all.filter(hasOpen).length, review: all.filter((r) => (r.to_review ?? 0) > 0).length, all: all.length };

  return (
    <div className="max-w-6xl mx-auto space-y-4">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100 flex items-center gap-2">
            <GitPullRequest size={22} className="text-emerald-400" />
            Pull requests
          </h1>
          <p className="text-sm text-zinc-400 mt-1">Your repositories and their pull requests. Pick a repository, then open a pull request to see its changes, review it and merge it.</p>
        </div>
        <RefreshButton onClick={refresh} busy={loadingRepos} disabled={!ready} title="Read the repositories and pull requests again" />
      </div>

      <div className="w-72 max-w-full">
        <label className="block text-xs text-zinc-500 mb-1">Account</label>
        <Select
          value={account}
          onChange={setAccount}
          placeholder={accounts === null ? "Loading accounts…" : "No GitHub account signed in"}
          optionIcon={<GitHubIcon size={14} />}
          options={(accounts ?? []).map((a) => ({ value: a.key, label: `${a.label} · ${a.source}` }))}
        />
      </div>

      {accounts === null && (
        <p className="flex items-center gap-2 text-sm text-zinc-400" role="status">
          <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading your GitHub accounts…
        </p>
      )}
      {accountsError && (
        <p role="alert" className="text-sm text-red-400">
          {accountsError}
        </p>
      )}
      {accounts && accounts.length === 0 && !accountsError && <p className="text-sm text-amber-300/90">Sign in on the GitHub Auth page (or with `gh auth login`) to see pull requests.</p>}

      {ready && (
        <div className="grid gap-4 lg:grid-cols-[19rem_minmax(0,1fr)] items-start">
          <section className="min-w-0 rounded-xl border border-zinc-700/50 bg-zinc-900/40 p-3 space-y-2.5" aria-label="Repositories">
            <h2 className="text-xs uppercase tracking-wider text-zinc-500">
              Repositories{repos ? ` · ${all.length}${repos.truncated ? "+" : ""}` : ""}
            </h2>
            <div className="relative">
              <SearchIcon size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500 pointer-events-none" />
              <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Filter repositories" aria-label="Filter repositories" className="!py-1.5 pl-8 text-sm" />
            </div>
            <Segmented
              label="Which repositories"
              value={repoFilter}
              onChange={(v) => setRepoFilter(v)}
              options={REPO_FILTERS.map((f) => ({ value: f.value, label: repos ? `${f.label} ${counts[f.value]}` : f.label }))}
            />
            {reposError && (
              <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs break-words">
                {reposError}
              </p>
            )}
            {!repos && loadingRepos && (
              <p className="flex items-center gap-2 text-xs text-zinc-400" role="status">
                <Loader2 size={13} className="animate-spin text-emerald-400" /> Reading @{login}'s repositories…
              </p>
            )}
            {repos && all.length === 0 && <p className="text-xs text-zinc-500">@{login} has no repositories.</p>}
            {repos && all.length > 0 && visible.length === 0 && (
              <div className="text-xs text-zinc-500 space-y-2">
                <p>
                  {q
                    ? `No repository matches “${query.trim()}”.`
                    : repoFilter === "review"
                      ? `Nobody is asking @${login} for a review.`
                      : `None of @${login}'s ${all.length} repositories has an open pull request.`}
                </p>
                {!q && repoFilter !== "all" && (
                  <Button size="sm" variant="secondary" onClick={() => setRepoFilter("all")}>
                    Show all repositories
                  </Button>
                )}
              </div>
            )}
            <div className="space-y-1 max-h-[24rem] lg:max-h-[calc(100vh-19rem)] overflow-y-auto -mr-1 pr-1">
              {visible.map((r) => {
                const on = current?.repo === r.repo;
                const [o, n] = r.repo.split("/");
                return (
                  <button
                    key={r.repo}
                    type="button"
                    data-pull-repo={r.repo}
                    aria-current={on ? "true" : undefined}
                    onClick={() => setPicked((m) => ({ ...m, [account]: r.repo }))}
                    title={r.affiliated ? r.repo : `${r.repo} — someone else's repository: you have pull requests there`}
                    className={`w-full text-left px-2.5 py-2 rounded-lg border transition-colors cursor-pointer min-w-0 ${
                      on ? "bg-emerald-500/10 border-emerald-500/40" : "bg-zinc-800/40 border-transparent hover:border-zinc-600"
                    }`}
                  >
                    <span className="flex items-center gap-1.5 min-w-0 text-sm">
                      {r.private && <Lock size={12} className="text-zinc-500 shrink-0" aria-label="private" />}
                      <span className="truncate min-w-0">
                        <span className="text-zinc-500">{o}/</span>
                        <span className={on ? "text-zinc-100 font-medium" : "text-zinc-200"}>{n}</span>
                      </span>
                      {r.archived && <Archive size={12} className="text-zinc-500 shrink-0" aria-label="archived" />}
                    </span>
                    <span className="flex items-center gap-1 flex-wrap mt-1 text-[11px]">
                      {r.open !== null && <span className={r.open > 0 ? "text-emerald-300" : "text-zinc-500"}>{r.open} open</span>}
                      {(r.to_review ?? 0) > 0 && <span className="px-1.5 rounded bg-amber-500/15 text-amber-300">{r.to_review} to review</span>}
                      {(r.mine ?? 0) > 0 && <span className="px-1.5 rounded bg-sky-500/15 text-sky-300">{r.mine} yours</span>}
                    </span>
                  </button>
                );
              })}
            </div>
            {repos && (repos.notes.length > 0 || repos.truncated) && (
              <ul className="text-[11px] text-zinc-500 space-y-0.5">
                {repos.truncated && <li>Only the {all.length} most recently pushed repositories are listed.</li>}
                {repos.notes.map((n) => (
                  <li key={n}>{n}</li>
                ))}
              </ul>
            )}
          </section>

          <section className="min-w-0 space-y-3" aria-label="Pull requests of the repository">
            {current ? (
              <>
                <div className="flex items-center gap-2 flex-wrap min-w-0">
                  <h2 className="text-base font-semibold text-zinc-100 truncate min-w-0" title={current.repo}>
                    <span className="text-zinc-500 font-normal">{owner}/</span>
                    {name}
                  </h2>
                  {current.private && <Badge>private</Badge>}
                  {current.archived && <Badge variant="warning">archived</Badge>}
                  {current.permission && <Badge variant={current.permission === "admin" || current.permission === "maintain" || current.permission === "write" ? "success" : "default"}>{current.permission}</Badge>}
                  <Button variant="ghost" size="sm" onClick={() => openOnGitHub(`https://github.com/${current.repo}/pulls`)} title="Open on GitHub">
                    <ExternalLink size={13} />
                  </Button>
                </div>
                <Segmented label="Whose pull requests" value={whose} onChange={(v) => setWhose(v)} options={WHOSE} />
                <PullList
                  key={`${account} ${current.repo}`}
                  account={account}
                  source={whose === "everyone" ? { kind: "repo", owner, name } : { kind: "search", scope: whose, repo: current.repo }}
                  state={state}
                  onState={(s) => setState(s)}
                  onOpen={setOpen}
                  reloadKey={reload}
                  hideRefresh
                />
              </>
            ) : (
              repos && visible.length > 0 && <p className="text-sm text-zinc-500">Pick a repository.</p>
            )}
          </section>
        </div>
      )}

      {open && (
        <PullPanel
          account={account}
          repo={open.repo}
          number={open.number}
          onClose={() => setOpen(null)}
          onChanged={() => {
            setReload((n) => n + 1);
            void loadRepos(true);
          }}
        />
      )}
    </div>
  );
}
