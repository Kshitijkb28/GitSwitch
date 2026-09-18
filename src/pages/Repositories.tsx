import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  BookMarked,
  Loader2,
  RefreshCw,
  Search,
  Lock,
  Globe,
  Download,
  FolderOpen,
  Copy,
  ChevronLeft,
  ChevronRight,
  AlertTriangle,
  X,
} from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Input } from "../components/Input";
import { Select } from "../components/Select";
import { Checkbox } from "../components/Checkbox";
import { GitHubIcon } from "../components/GitHubIcon";
import { useToast } from "../components/Toast";
import { usePersistedState } from "../lib/persist";
import { useRefreshOnFocus } from "../lib/focus";
import type { Profile } from "../types/profile";
import * as api from "../lib/api";

const PAGE_SIZE = 25;

// Listings survive navigating away and back within a session; Refresh re-fetches.
const listingCache = new Map<string, { listing: api.RepoListing; fetchedAt: number }>();
// A listing can take a while for big accounts — never ask GitHub twice at once
// for the same account (double mounts, quick account flips back and forth).
const inflight = new Map<string, Promise<api.RepoListing>>();

// The local scan is cheap but not free (it walks the profile folders), and
// several things ask for it at once — mount, account change, window focus.
// One scan serves all of them.
let localInflight: Promise<api.LocalClones> | null = null;
function scanLocalClones(): Promise<api.LocalClones> {
  if (!localInflight) {
    localInflight = api.localCloneIndex().finally(() => {
      localInflight = null;
    });
  }
  return localInflight;
}

function fetchListing(key: string, force: boolean): Promise<api.RepoListing> {
  const running = inflight.get(key);
  if (running && !force) return running;
  const p = api.listRemoteRepos(key).finally(() => {
    if (inflight.get(key) === p) inflight.delete(key);
  });
  inflight.set(key, p);
  return p;
}

function timeAgo(iso: string | null): string {
  if (!iso) return "never pushed";
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 3600) return `pushed ${Math.max(1, Math.floor(secs / 60))}m ago`;
  if (secs < 86400) return `pushed ${Math.floor(secs / 3600)}h ago`;
  if (secs < 86400 * 30) return `pushed ${Math.floor(secs / 86400)}d ago`;
  if (secs < 86400 * 365) return `pushed ${Math.floor(secs / (86400 * 30))}mo ago`;
  return `pushed ${Math.floor(secs / (86400 * 365))}y ago`;
}

function shortPath(p: string): string {
  const parts = p.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts.slice(-2).join("/");
}

export function Repositories() {
  const toast = useToast();
  const navigate = useNavigate();

  const [accounts, setAccounts] = useState<api.RepoAccount[] | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [account, setAccount] = usePersistedState("repos.account", "");
  const [listing, setListing] = useState<api.RepoListing | null>(null);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [search, setSearch] = usePersistedState("repos.search", "");
  const [owner, setOwner] = usePersistedState("repos.owner", "");
  const [visibility, setVisibility] = usePersistedState<"all" | "private" | "public">("repos.visibility", "all");
  const [hideArchived, setHideArchived] = usePersistedState("repos.hideArchived", true);
  const [hideForks, setHideForks] = usePersistedState("repos.hideForks", false);
  const [notClonedOnly, setNotClonedOnly] = usePersistedState("repos.notClonedOnly", false);
  const [sort, setSort] = usePersistedState<"pushed" | "name">("repos.sort", "pushed");
  const [page, setPage] = useState(0);
  const loadSeq = useRef(0);

  // Which repos are on disk is local, cheap and changes behind the app's back
  // (a folder deleted in Finder), so it is re-read on every visit and whenever
  // the window regains focus — never taken from the cached GitHub listing.
  const [local, setLocal] = useState<api.LocalClones | null>(null);
  const refreshLocal = useCallback(() => {
    scanLocalClones().then(setLocal).catch(() => setLocal(null));
  }, []);
  useEffect(refreshLocal, [refreshLocal]);
  useRefreshOnFocus(refreshLocal);

  /** Fresh local answers win over whatever the listing said when it was fetched. */
  const localPathOf = (r: api.RemoteRepo) =>
    local ? local.clones[r.full_name.toLowerCase()] ?? null : r.local_path;
  const cloneUrlOf = (r: api.RemoteRepo) => {
    const orgUser = local?.org_ssh_users[r.owner.toLowerCase()];
    return orgUser ? `${orgUser}@github.com:${r.owner}/${r.name}.git` : r.clone_url;
  };

  useEffect(() => {
    api.getProfiles().then(setProfiles).catch(() => setProfiles([]));
    api.repoAccounts()
      .then((a) => {
        setAccounts(a);
        // Keep the remembered account only if it's still signed in.
        setAccount((prev) => (a.some((x) => x.key === prev) ? prev : a[0]?.key ?? ""));
      })
      .catch(() => setAccounts([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function load(key: string, force = false) {
    if (!key) return;
    const cached = listingCache.get(key);
    if (cached && !force) {
      setListing(cached.listing);
      setFetchedAt(cached.fetchedAt);
      setError(null);
      return;
    }
    const seq = ++loadSeq.current;
    setLoading(true);
    setError(null);
    try {
      const l = await fetchListing(key, force);
      if (loadSeq.current !== seq) return;
      const now = Date.now();
      listingCache.set(key, { listing: l, fetchedAt: now });
      setListing(l);
      setFetchedAt(now);
    } catch (e) {
      if (loadSeq.current !== seq) return;
      setError(String(e));
      setListing(null);
    } finally {
      if (loadSeq.current === seq) setLoading(false);
    }
  }

  useEffect(() => {
    if (account) load(account);
    refreshLocal();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account]);

  // Any filter change starts from the first page.
  useEffect(() => setPage(0), [account, search, owner, visibility, hideArchived, hideForks, notClonedOnly, sort]);

  const owners = useMemo(
    () => Array.from(new Set((listing?.repos ?? []).map((r) => r.owner))).sort((a, b) => a.localeCompare(b)),
    [listing]
  );
  // A remembered owner filter from another account shouldn't hide everything.
  useEffect(() => {
    if (listing && owner && !owners.includes(owner)) setOwner("");
  }, [listing, owner, owners, setOwner]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    const rows = (listing?.repos ?? []).filter(
      (r) =>
        (!q || r.full_name.toLowerCase().includes(q) || (r.description ?? "").toLowerCase().includes(q)) &&
        (!owner || r.owner === owner) &&
        (visibility === "all" || (visibility === "private") === r.private) &&
        (!hideArchived || !r.archived) &&
        (!hideForks || !r.fork) &&
        (!notClonedOnly || !localPathOf(r))
    );
    if (sort === "name") rows.sort((a, b) => a.full_name.localeCompare(b.full_name));
    else rows.sort((a, b) => (b.pushed_at ?? "").localeCompare(a.pushed_at ?? ""));
    return rows;
  }, [listing, local, search, owner, visibility, hideArchived, hideForks, notClonedOnly, sort]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const safePage = Math.min(page, pageCount - 1);
  const visible = filtered.slice(safePage * PAGE_SIZE, (safePage + 1) * PAGE_SIZE);
  const profileName = (id: string | null) => profiles.find((p) => p.id === id)?.name ?? null;


  function suggestionFor(r: api.RemoteRepo): string | null {
    const fromDisk = local?.owner_profiles[r.owner.toLowerCase()];
    if (fromDisk) return fromDisk;
    if (!local && r.suggested_profile_id) return r.suggested_profile_id;
    // The account's own profile only ever applies to the account's own repos.
    return listing && r.owner.toLowerCase() === listing.login.toLowerCase()
      ? listing.suggested_profile_id
      : null;
  }

  function cloneRepo(r: api.RemoteRepo) {
    const params = new URLSearchParams({ url: cloneUrlOf(r), from: r.full_name, mode: "full" });
    const s = suggestionFor(r);
    if (s) params.set("cloneAs", s);
    navigate(`/sparse?${params.toString()}`);
  }

  async function openInHistory(path: string) {
    // The row may be a moment out of date — check before acting on the folder.
    // Only an explicit "no" blocks: if the check itself fails, let the user
    // through rather than refusing to open a folder that is probably there.
    const stillThere = await api.pathExists(path).catch(() => true);
    if (stillThere === false) {
      toast.error("That folder no longer exists — the list has been updated");
      refreshLocal();
      return;
    }
    try {
      localStorage.setItem("gitswitch:history.repo", JSON.stringify(path));
      localStorage.setItem("gitswitch:history.account", JSON.stringify(""));
      localStorage.setItem("gitswitch:history.rev", JSON.stringify("--all"));
    } catch {
      /* storage unavailable — History will simply open on its default repo */
    }
    navigate("/history");
  }

  async function copyLink(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      toast.success("SSH link copied");
    } catch {
      setError(`Couldn't copy — the link is ${url}`);
    }
  }

  const noAccounts = accounts !== null && accounts.length === 0;
  const clonedCount = (listing?.repos ?? []).filter((r) => localPathOf(r)).length;

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">Repositories</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Everything your GitHub accounts can reach — clone any of them as the right account.
          </p>
        </div>
        {!noAccounts && (
          <div className="flex items-center gap-2 flex-wrap min-w-0">
            <div className="w-64 max-w-full">
              <Select
                value={account}
                onChange={setAccount}
                placeholder={accounts === null ? "Loading accounts…" : "Pick an account…"}
                optionIcon={<GitHubIcon size={14} />}
                options={(accounts ?? []).map((a) => ({ value: a.key, label: a.label }))}
              />
            </div>
            <Button
              variant="secondary"
              onClick={() => load(account, true)}
              disabled={!account || loading}
              className="min-w-[8rem]" // fits "Loading…" (≈120px) so the swap doesn't shift the picker
            >
              <RefreshCw size={16} className={loading ? "animate-spin" : ""} />
              {loading ? "Loading…" : "Refresh"}
            </Button>
          </div>
        )}
      </div>

      {noAccounts && (
        <Card className="text-center py-10">
          <BookMarked size={32} className="mx-auto text-zinc-600 mb-3" />
          <h3 className="text-lg font-medium text-zinc-300">No GitHub accounts signed in</h3>
          <p className="text-sm text-zinc-500 mt-1 max-w-md mx-auto">
            Sign in with the GitHub CLI (<span className="font-mono">gh auth login</span>) or on the
            GitHub Auth page, and your repositories will show up here.
          </p>
          <Button className="mt-4" variant="secondary" onClick={() => navigate("/github")}>
            <GitHubIcon size={16} />
            Go to GitHub Auth
          </Button>
        </Card>
      )}

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      {listing && listing.sso_hidden_orgs > 0 && (
        <div className="flex items-start gap-2.5 px-4 py-3 rounded-lg bg-amber-500/10 border border-amber-500/30 text-sm text-amber-200">
          <AlertTriangle size={16} className="mt-0.5 shrink-0" />
          <span className="min-w-0">
            Repositories from {listing.sso_hidden_orgs} organization
            {listing.sso_hidden_orgs === 1 ? " are" : "s are"} hidden: this sign-in isn't authorized
            for {listing.sso_hidden_orgs === 1 ? "its" : "their"} single sign-on yet. Authorize it for
            {listing.sso_hidden_orgs === 1 ? " that organization" : " those organizations"} on GitHub,
            then press Refresh.
          </span>
        </div>
      )}
      {listing?.truncated && (
        <div className="px-4 py-3 rounded-lg bg-zinc-800/60 border border-zinc-700/50 text-sm text-zinc-400">
          This account can reach more than {listing.repos.length.toLocaleString()} repositories — only the
          most recently pushed ones are listed. Use the search to narrow things down.
        </div>
      )}

      {account && !noAccounts && (
        <Card>
          <div className="flex flex-col lg:flex-row gap-3 lg:items-center">
            <div className="relative flex-1 min-w-0">
              <Search size={15} className="absolute left-3 top-1/2 -translate-y-1/2 text-zinc-500 pointer-events-none" />
              <Input
                placeholder="Search name or description…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className="pl-9"
              />
              {search && (
                <button
                  onClick={() => setSearch("")}
                  className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-zinc-500 hover:text-zinc-300 cursor-pointer"
                  aria-label="Clear search"
                >
                  <X size={14} />
                </button>
              )}
            </div>
            <div className="w-full lg:w-52">
              <Select
                value={owner}
                onChange={setOwner}
                options={[{ value: "", label: "All owners" }, ...owners.map((o) => ({ value: o, label: o }))]}
              />
            </div>
            <div className="w-full lg:w-44">
              <Select
                value={sort}
                onChange={(v) => setSort(v as "pushed" | "name")}
                options={[
                  { value: "pushed", label: "Recently pushed" },
                  { value: "name", label: "Name (A–Z)" },
                ]}
              />
            </div>
          </div>

          <div className="flex items-center gap-x-5 gap-y-2 flex-wrap mt-3">
            <div className="inline-flex rounded-lg bg-zinc-800/80 p-0.5">
              {(["all", "private", "public"] as const).map((v) => (
                <button
                  key={v}
                  type="button"
                  onClick={() => setVisibility(v)}
                  className={`px-3 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer ${
                    visibility === v ? "bg-zinc-700 text-emerald-400" : "text-zinc-400 hover:text-zinc-200"
                  }`}
                >
                  {v === "all" ? "All" : v === "private" ? "Private" : "Public"}
                </button>
              ))}
            </div>
            {[
              { label: "Hide archived", value: hideArchived, set: setHideArchived },
              { label: "Hide forks", value: hideForks, set: setHideForks },
              { label: "Not cloned yet", value: notClonedOnly, set: setNotClonedOnly },
            ].map((f) => (
              <label key={f.label} className="inline-flex items-center gap-2 text-xs text-zinc-400 cursor-pointer select-none">
                <Checkbox checked={f.value} onChange={f.set} />
                {f.label}
              </label>
            ))}
          </div>

          <div className="flex items-center justify-between gap-3 flex-wrap mt-4 mb-2 text-xs text-zinc-500">
            <span>
              {listing ? (
                <>
                  {filtered.length === 0
                    ? "No matching repositories"
                    : `Showing ${(safePage * PAGE_SIZE + 1).toLocaleString()}–${Math.min(
                        (safePage + 1) * PAGE_SIZE,
                        filtered.length
                      ).toLocaleString()} of ${filtered.length.toLocaleString()}`}
                  {filtered.length !== listing.repos.length &&
                    ` (${listing.repos.length.toLocaleString()} in total)`}
                  {" · "}
                  {clonedCount.toLocaleString()} already on this Mac · signed in as{" "}
                  <span className="font-mono text-zinc-400">{listing.login}</span>
                </>
              ) : loading ? (
                "Loading repositories…"
              ) : (
                ""
              )}
            </span>
            {fetchedAt && !loading && (
              <span className="text-zinc-600">
                {Date.now() - fetchedAt < 60000
                  ? "updated just now"
                  : `updated ${Math.round((Date.now() - fetchedAt) / 60000)}m ago`}
              </span>
            )}
          </div>

          {loading && !listing && (
            <div className="flex items-center justify-center gap-2 py-12 text-sm text-zinc-400">
              <Loader2 size={16} className="animate-spin text-emerald-400" />
              Loading every repository this account can reach…
            </div>
          )}

          <div className="divide-y divide-zinc-800/70">
            {visible.map((r) => {
              const suggestion = profileName(suggestionFor(r));
              const localPath = localPathOf(r);
              return (
                <div key={r.full_name} className="flex items-start gap-3 py-3 min-w-0">
                  <span className="mt-0.5 shrink-0 text-zinc-500" title={r.private ? "Private" : "Public"}>
                    {r.private ? <Lock size={15} /> : <Globe size={15} />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5 flex-wrap min-w-0">
                      <span className="text-sm text-zinc-200 truncate min-w-0" title={r.full_name}>
                        <span className="text-zinc-500">{r.owner} / </span>
                        <span className="font-medium">{r.name}</span>
                      </span>
                      {r.archived && <Badge variant="warning">archived</Badge>}
                      {r.fork && <Badge variant="default">fork</Badge>}
                      <Badge variant={r.permission === "read" ? "default" : "success"}>{r.permission}</Badge>
                    </div>
                    {r.description && (
                      <p className="text-xs text-zinc-500 truncate mt-0.5" title={r.description}>
                        {r.description}
                      </p>
                    )}
                    <p className="text-[11px] text-zinc-600 mt-1 truncate">
                      {timeAgo(r.pushed_at)}
                      {r.default_branch && <> · {r.default_branch}</>}
                      {localPath ? (
                        <span className="text-emerald-400/80" title={localPath}>
                          {" "}· cloned at {shortPath(localPath)}
                        </span>
                      ) : (
                        suggestion && <> · clones as {suggestion}</>
                      )}
                    </p>
                  </div>
                  <div className="flex items-center gap-1.5 shrink-0">
                    <button
                      onClick={() => copyLink(cloneUrlOf(r))}
                      title={`Copy ${cloneUrlOf(r)}`}
                      className="p-2 rounded-lg text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800 transition-colors cursor-pointer"
                      aria-label="Copy SSH link"
                    >
                      <Copy size={14} />
                    </button>
                    {localPath ? (
                      <Button size="sm" variant="secondary" onClick={() => openInHistory(localPath)}>
                        <FolderOpen size={14} />
                        Open
                      </Button>
                    ) : (
                      <Button size="sm" onClick={() => cloneRepo(r)}>
                        <Download size={14} />
                        Clone
                      </Button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>

          {filtered.length > PAGE_SIZE && (
            <div className="flex items-center justify-between gap-3 flex-wrap pt-4 mt-2 border-t border-zinc-800">
              <Button variant="secondary" size="sm" onClick={() => setPage(safePage - 1)} disabled={safePage === 0}>
                <ChevronLeft size={14} />
                Previous
              </Button>
              <span className="text-xs text-zinc-500">
                Page {safePage + 1} of {pageCount}
              </span>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setPage(safePage + 1)}
                disabled={safePage >= pageCount - 1}
              >
                Next
                <ChevronRight size={14} />
              </Button>
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
