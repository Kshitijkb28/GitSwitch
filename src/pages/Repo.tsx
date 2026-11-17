import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { NavLink, useNavigate, useParams } from "react-router-dom";
import { GitBranch, Box, GitGraph, FileDiff, GitPullRequest, GitPullRequestArrow, ScanSearch, SquareTerminal, Folder } from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import { NotARepositoryCard, type FolderEntry } from "../components/repo/NotARepositoryCard";
import { StatusStrip, type RepoActions, type StripActions } from "../components/repo/StatusStrip";
import { WorkspaceLine } from "../components/repo/WorkspaceLine";
import { ChangesTab } from "../components/repo/ChangesTab";
import { HistoryTab } from "../components/repo/HistoryTab";
import { SubmodulesTab } from "../components/repo/SubmodulesTab";
import { UpdateDrawer } from "../components/repo/UpdateDrawer";
import { FindReposDialog } from "../components/FindReposDialog";
import { GitHubTab } from "../components/repo/GitHubTab";
import { markMoved, reconcilePlan } from "../lib/updatePlan";
import { useRepoSelection } from "../lib/repoSelection";
import { usePersistedState } from "../lib/persist";
import { baseName, canonicalJoin, normPath } from "../lib/paths";
import { useRefreshOnFocus } from "../lib/focus";
import { useGitJob } from "../lib/gitJobs";
import { openConsole } from "../lib/console";
import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import * as api from "../lib/api";
import type { PullMode, RepoRef, RepoStatus, SubmoduleInfo } from "../lib/api";

type Tab = "changes" | "history" | "submodules" | "github";
const TABS: Tab[] = ["changes", "history", "submodules", "github"];

/** The first real repository of a list (folders that hold none come after), or the first entry. */
function firstRepo(list: RepoRef[]): RepoRef | undefined {
  return list.find((r) => r.kind !== "folder") ?? list[0];
}

/** "staging ↑2 ↓1" — the submodule's own branch against its own upstream. */
function subLabel(m: SubmoduleInfo): string {
  const parts = [m.path];
  if (m.own_ahead > 0) parts.push(`↑${m.own_ahead}`);
  if (m.own_behind > 0) parts.push(`↓${m.own_behind}`);
  return parts.join(" ");
}

/**
 * One page for one repository: the working tree, the history and the
 * submodules as tabs under one picker row and one status strip. The two pages
 * this replaces lived side by side with their own pickers; a submodule opened
 * on one was unknown to the other, and nothing said where you stood until you
 * had read both.
 */
export function Repo() {
  const navigate = useNavigate();
  const { tab: tabParam } = useParams<{ tab: string }>();
  const tab: Tab = TABS.includes(tabParam as Tab) ? (tabParam as Tab) : "changes";
  const sel = useRepoSelection();
  const toast = useToast();

  const [repos, setRepos] = useState<RepoRef[]>([]);
  const [loadingRepos, setLoadingRepos] = useState(true);
  /** Folders opened by hand, outside every profile's folders. */
  const [extraRepos, setExtraRepos] = useState<string[]>([]);
  /** What each folder opened by hand is to git, once asked: one that holds no repository gets the card, not the tabs. */
  const [extraStates, setExtraStates] = useState<Record<string, api.FolderState>>({});
  /** Find repositories: clones no profile's folders hold, added to the right profile. */
  const [findOpen, setFindOpen] = useState(false);
  const [subs, setSubs] = useState<SubmoduleInfo[]>([]);
  /** Every gitlink, populated or not — whether the repository has submodules at all. */
  const [gitlinkPaths, setGitlinkPaths] = useState<string[]>([]);
  /** The repository the gitlink list above was read for. */
  const [gitlinksFor, setGitlinksFor] = useState("");
  const gitlinks = gitlinkPaths.length;
  const [status, setStatus] = useState<RepoStatus | null>(null);
  /** The other copies of this project on this machine, read with the repository and on Refresh. */
  const [workspace, setWorkspace] = useState<api.WorkspaceInfo | null>(null);
  /** Every listener on this machine, for the workspace line's count; null while unread. */
  const [listeners, setListeners] = useState<api.Listener[] | null>(null);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const actionsRef = useRef<RepoActions | null>(null);
  const [peek, setPeek] = useState<api.RemotePeek | null>(null);
  const [peeking, setPeeking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Bumped when the repository visibly moved, so the History tab re-reads. */
  const [reloadKey, setReloadKey] = useState(0);
  /** Bumped by Refresh only (with reloadKey): a hidden tab reads again at once rather than when shown. */
  const [refreshKey, setRefreshKey] = useState(0);
  // The ports listing is read again on Refresh, not on every commit: what
  // listens doesn't move with HEAD, and on Windows each read starts PowerShell.
  const [portsKey, setPortsKey] = useState(0);
  const lastMoveKey = useRef("");
  /** Whether the last status read came within ten seconds of a fetch. */
  const lastFresh = useRef(false);
  /** The Update drawer: preview, blockers, Update now. */
  // The Update drawer, and the repository it was opened for: it shows only
  // while that repository is the one on screen.
  const [drawerFor, setDrawerFor] = useState<string | null>(null);
  const drawer = drawerFor !== null && drawerFor === sel.effectivePath;
  const pathRef = useRef("");

  const [pullMode, setPullMode] = usePersistedState<PullMode>("changes.pullMode", "ff-only");
  // On by default: with LFS filters configured git already downloads large
  // files on pull, so this makes every repo behave the way people expect.
  const [pullLfs, setPullLfs] = usePersistedState("changes.pullLfs", true);
  // Off by default: stashing around a rebase can leave conflicts behind, so
  // it is a choice, not a surprise.
  const [pullAutostash, setPullAutostash] = usePersistedState("changes.pullAutostash", false);

  const job = useGitJob(sel.effectivePath);

  const loadRepos = useCallback(() => {
    let cancelled = false;
    api
      .historyListRepos()
      .then((r) => {
        if (!cancelled) setRepos(r);
      })
      .catch((e) => !cancelled && setError(String(e)))
      .finally(() => !cancelled && setLoadingRepos(false));
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(loadRepos, [loadRepos]);
  // Repos get cloned and deleted outside the app; re-scan on return.
  useRefreshOnFocus(loadRepos, 15000);

  const accounts = useMemo(
    () =>
      Array.from(
        new Map(repos.map((r) => [r.profile_id, { id: r.profile_id, name: r.profile_name, email: r.profile_email }])).values()
      ),
    [repos]
  );
  const accountEmail = accounts.find((a) => a.id === sel.account)?.email ?? "";
  const visibleRepos = useMemo(
    () => (sel.account ? repos.filter((r) => r.profile_id === sel.account) : repos),
    [sel.account, repos]
  );

  // A remembered repository that is in no profile folder (a submodule opened as
  // its own repository, a folder opened by hand) stays selectable — while it
  // is on disk. One deleted since gives way to the first repository listed,
  // or the page (and the terminal it hands its folder to) would work in a
  // folder that is not there.
  useEffect(() => {
    if (loadingRepos || !sel.path) return;
    if (repos.some((r) => r.path === sel.path)) return;
    const path = sel.path;
    let stale = false;
    api
      .pathExists(path)
      .then((exists) => {
        if (stale) return;
        if (exists === false) {
          const next = firstRepo(visibleRepos)?.path ?? firstRepo(repos)?.path;
          if (next) sel.setPath(next);
          return;
        }
        setExtraRepos((l) => (l.includes(path) ? l : [...l, path]));
      })
      .catch(() => {
        if (!stale) setExtraRepos((l) => (l.includes(path) ? l : [...l, path]));
      });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingRepos, repos, sel.path]);

  // No repository yet: take the first one the scan finds — a real repository
  // before a folder that holds none.
  useEffect(() => {
    if (loadingRepos || sel.path || repos.length === 0) return;
    const first = firstRepo(repos);
    if (first) sel.setPath(first.path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingRepos, repos, sel.path]);

  // If the chosen account doesn't own the current repo, move to one it does.
  useEffect(() => {
    if (!sel.account || !repos.length) return;
    const owned = repos.filter((r) => r.profile_id === sel.account);
    if (owned.length && !owned.some((r) => r.path === sel.path) && !extraRepos.includes(sel.path)) {
      const first = firstRepo(owned);
      if (first) sel.setPath(first.path);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel.account, repos, sel.path]);

  // Folders opened by hand: asked once each what they are to git.
  useEffect(() => {
    const unknown = extraRepos.filter((p) => !(p in extraStates));
    if (!unknown.length) return;
    let cancelled = false;
    void Promise.all(
      unknown.map((p) =>
        api
          .folderState(p)
          .then((s) => [p, s] as const)
          // Unanswerable: treated as a repository, as before (the tabs say what is wrong).
          .catch(() => [p, { kind: "repo" } as api.FolderState] as const)
      )
    ).then((pairs) => {
      if (!cancelled) setExtraStates((m) => ({ ...m, ...Object.fromEntries(pairs) }));
    });
    return () => {
      cancelled = true;
    };
  }, [extraRepos, extraStates]);

  /**
   * The folder on screen when it is no repository: a profile folder that holds
   * none (listed as `folder`), or a folder opened by hand that holds none. A
   * subfolder of a repository opened by hand is not one — git works there.
   */
  const folder = useMemo<FolderEntry | null>(() => {
    if (!sel.path || sel.sub) return null;
    // Made a repository here a moment ago: the listing still says "folder"
    // until the profiles' folders are scanned again.
    if (extraStates[sel.path]?.kind === "repo") return null;
    const listed = repos.find((r) => r.path === sel.path);
    if (listed) {
      return listed.kind === "folder"
        ? { path: listed.path, name: listed.name, profileId: listed.profile_id, profileName: listed.profile_name, profileEmail: listed.profile_email, inside: listed.inside ?? null }
        : null;
    }
    return extraStates[sel.path]?.kind === "no-repo" ? { path: sel.path, name: baseName(sel.path), profileId: null, profileName: null, profileEmail: null, inside: null } : null;
  }, [repos, extraStates, sel.path, sel.sub]);

  /** Make the folder on screen a repository, then show it as one. */
  const makeRepository = async (entry: FolderEntry) => {
    const made = await api.repoInit(entry.path);
    toast.success(`${entry.name} is now a git repository`, [
      `Branch ${made.branch}, with no commits yet`,
      entry.profileName ? `Commits here use ${entry.profileName}'s identity (${entry.profileEmail})` : "",
      "Next: make the first commit on the Changes tab",
    ]);
    setExtraStates((m) => ({ ...m, [entry.path]: { kind: "repo" } }));
    loadRepos();
    go("changes");
  };

  // The repository's populated, mapped submodules — each can be the scope. A
  // remembered submodule that no longer exists falls back to the repository.
  useEffect(() => {
    if (!sel.path) {
      setSubs([]);
      setGitlinkPaths([]);
      setGitlinksFor("");
      return;
    }
    let cancelled = false;
    api
      .changesSubmodules(sel.path)
      .then((list) => {
        if (cancelled) return;
        const usable = list.filter((m) => m.initialised && m.listed && m.gitdir_valid);
        setGitlinkPaths(list.map((m) => m.path));
        setGitlinksFor(sel.path);
        setSubs(usable);
        // A scope that is no gitlink at all is stale (the submodule was removed);
        // one that merely is not initialised or mapped stays — "Open as its own
        // repository" has always worked on any folder.
        if (sel.sub && !list.some((m) => m.path === sel.sub)) sel.setSub("");
      })
      .catch(() => {
        if (!cancelled) {
          setSubs([]);
          setGitlinkPaths([]);
        }
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel.path, reloadKey]);

  // The workspace: this checkout among the other copies of its project. A
  // folder that is no repository (or was deleted) simply has none.
  useEffect(() => {
    if (!sel.path) {
      setWorkspace(null);
      return;
    }
    let cancelled = false;
    api
      .workspaceInfo(sel.path)
      .then((w) => {
        if (!cancelled) setWorkspace(w);
      })
      .catch(() => {
        if (!cancelled) setWorkspace(null);
      });
    return () => {
      cancelled = true;
    };
  }, [sel.path, reloadKey]);

  // What listens from this checkout, for the workspace line — the same listing
  // the Ports page shows. Unreadable (or not a repository) means unread, not zero.
  useEffect(() => {
    if (!sel.path) {
      setListeners(null);
      return;
    }
    let cancelled = false;
    api
      .portsList()
      .then((r) => {
        if (!cancelled) setListeners(r ? r.rows : null);
      })
      .catch(() => {
        if (!cancelled) setListeners(null);
      });
    return () => {
      cancelled = true;
    };
  }, [sel.path, portsKey]);

  // The Changes tab reads the status; the strip shows it, and the History tab
  // re-reads when the repository actually moved — not on every status refresh.
  const onStatus = useCallback((s: RepoStatus | null, loading: boolean) => {
    setStatus(s);
    setLoadingStatus(loading);
    // A plan made for another HEAD or upstream is stale: drop it — judged on
    // a settled reading only. While a refresh is in flight the status here is
    // still the previous one, and a plan made a moment ago for the HEAD the
    // backend sees would be thrown away against it.
    if (!loading) reconcilePlan(pathRef.current, s);
    if (s) {
      const key = `${s.path}|${s.head_oid ?? ""}|${s.ahead}|${s.behind}|${s.upstream ?? ""}|${s.stash_count}|${s.conflicted_count}`;
      // A fetch a moment ago may have moved the remote refs: one re-read. The
      // flag going stale ten seconds later is not a move — re-reading on that
      // edge too doubled every fetch into two full-page reloads, hundreds of
      // git runs each on a repository with submodules.
      const fresh = s.last_fetch_secs !== null && s.last_fetch_secs < 10;
      const first = lastMoveKey.current === "";
      const moved = lastMoveKey.current !== key;
      const fetched = fresh && !lastFresh.current;
      if (!first && (moved || fetched)) setReloadKey((k) => k + 1);
      lastMoveKey.current = key;
      lastFresh.current = fresh;
    }
  }, []);
  const [actionsReady, setActionsReady] = useState(false);
  const onActions = useCallback((a: RepoActions) => {
    if (!actionsRef.current) setActionsReady(true);
    actionsRef.current = a;
  }, []);
  // The strip calls through the ref at click time. The Changes tab registers
  // fresh closures after every render, so a copy taken during this page's
  // render would be one render stale — an autostash switched on a moment ago
  // would not be seen by Pull.
  const stripActions = useMemo<StripActions>(
    () => ({
      fetch: () => actionsRef.current?.fetch(),
      pull: () => actionsRef.current?.pull(),
      push: () => actionsRef.current?.push(),
      branches: () => actionsRef.current?.branches(),
      refresh: () => {
        // Everything a fresh visit reads: the repository, its submodules and
        // the history (reloadKey), and the list of repositories to pick from.
        actionsRef.current?.refresh();
        setReloadKey((k) => k + 1);
        setRefreshKey((k) => k + 1);
        setPortsKey((k) => k + 1);
        loadRepos();
      },
    }),
    [loadRepos]
  );

  /** ls-remote — asks the remote where its branch is, writes nothing at all. */
  async function doPeek() {
    if (!sel.effectivePath) return;
    setPeeking(true);
    setError(null);
    try {
      setPeek(await api.historyPeek(sel.effectivePath));
    } catch (e) {
      setError(String(e));
      setPeek(null);
    } finally {
      setPeeking(false);
    }
  }
  // A fetch makes the check's answer stale.
  useEffect(() => {
    if (job?.kind === "fetch" && job.running) setPeek(null);
  }, [job?.kind, job?.running]);
  useEffect(() => {
    setPeek(null);
  }, [sel.effectivePath]);

  const dirty = (status?.staged_count ?? 0) + (status?.unstaged_count ?? 0) > 0;
  const busy = job?.running ?? false;
  const changedCount = status ? status.staged_count + status.unstaged_count + status.untracked_count : 0;
  const movedCount = subs.filter((m) => m.state === "moved" || m.state === "moved-and-dirty").length;

  const go = (t: Tab) => navigate(`/repo/${t}`);
  useEffect(() => {
    if (tab === "submodules" && !sel.sub && sel.path && gitlinksFor === sel.path && gitlinkPaths.length === 0) {
      navigate("/repo/changes", { replace: true });
    }
  }, [tab, sel.sub, sel.path, gitlinksFor, gitlinkPaths.length, navigate]);
  pathRef.current = sel.effectivePath;
  // The drawer belongs to one repository: moving to another closes it.
  useEffect(() => {
    if (drawerFor !== null && drawerFor !== sel.effectivePath) setDrawerFor(null);
  }, [sel.effectivePath, drawerFor]);
  /**
   * A run on `repo` was refused because upstream moved: its plan is marked
   * stale; the drawer re-plans and says why — if that repository is the one
   * on screen. A run that outlived a repository switch marks only its own.
   */
  const onAssessAgain = (repo: string) => {
    markMoved(repo);
    if (repo === pathRef.current) setDrawerFor(repo);
  };
  /**
   * A paused update hands over: the superproject's conflicts are on the
   * Changes tab; a submodule's are inside it, so it becomes the scope first.
   */
  /**
   * Opens a submodule of the repository an update ran on. With no scope that
   * is a submodule of this repository, and it becomes the scope. From inside
   * a scope it is one level further down — and a nested repository can only
   * be the page's repository, never a scope — so it is opened as one.
   */
  const openSubOfScope = (p: string) => {
    if (!sel.sub) {
      sel.setSub(p);
      return;
    }
    const abs = canonicalJoin(sel.effectivePath, p);
    setExtraRepos((l) => (l.includes(abs) ? l : [...l, abs]));
    sel.setPath(abs);
  };
  const goToConflicts = (where: string) => {
    if (where.startsWith("submodule")) {
      const p = where.split(":")[1];
      if (p) openSubOfScope(p);
    }
    setDrawerFor(null);
    go("changes");
    setTimeout(() => document.getElementById("conflicts")?.scrollIntoView({ behavior: "smooth", block: "start" }), 200);
  };
  const runUpdate = (stash: boolean, bundles: boolean, fingerprint: [string, string][]) => {
    const a = actionsRef.current;
    if (!a) return;
    setDrawerFor(null);
    void a.update(sel.effectivePath, stash, bundles, fingerprint).finally(() => {
      setReloadKey((k) => k + 1);
      actionsRef.current?.refresh();
    });
  };
  /** A remedy button in the drawer, keyed on the backend's code. */
  const remedy = async (code: string, sub?: string) => {
    const a = actionsRef.current;
    const repo = sel.effectivePath;
    if (!a || !repo) return;
    switch (code) {
      case "record-pointers":
        await a.recordPointers(repo);
        break;
      case "reinit":
        if (sub) await a.initialise(repo, [sub]);
        break;
      case "stash-collides":
        await a.stashNow(repo);
        break;
      case "open":
        if (sub) {
          setDrawerFor(null);
          openSubOfScope(sub);
          go("changes");
        }
        return;
      case "untracked-in-the-way":
        setDrawerFor(null);
        go("changes");
        return;
      case "choose-branch":
        setDrawerFor(null);
        a.branches();
        return;
      default:
        return;
    }
    setReloadKey((k) => k + 1);
    actionsRef.current?.refresh();
  };
  /**
   * An operation the Submodules tab asked for. It runs in the Changes tab (the
   * job slot, the result card, the toasts) on the repository itself — so the
   * scope goes back to it first — and everything re-reads when it is done.
   */
  // An operation on the scoped repository — the repository, or a submodule
  // inside it whose own submodules the tab shows: the scope stays where it is.
  const runHere = (op: (a: RepoActions) => Promise<unknown>) => {
    const a = actionsRef.current;
    if (!a) return;
    void op(a).finally(() => {
      setReloadKey((k) => k + 1);
      actionsRef.current?.refresh();
    });
  };
  // Arriving on a tab re-reads the repository, as opening the old page did:
  // the terminal drawer or an operation on another tab may have moved HEAD.
  const firstTab = useRef(true);
  useEffect(() => {
    if (firstTab.current) {
      firstTab.current = false;
      return;
    }
    actionsRef.current?.refresh();
  }, [tab]);

  return (
    <div className="max-w-6xl mx-auto space-y-4">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">Repository</h1>
          <p className="text-sm text-zinc-400 mt-0.5">
            Your changes, its history and its submodules — as the account this folder belongs to.
          </p>
        </div>
        <div className="flex items-end gap-2 flex-wrap">
          <div className="w-44">
            <label className="block text-xs text-zinc-500 mb-1">Account</label>
            <Select
              value={sel.account}
              onChange={sel.setAccount}
              placeholder="All accounts"
              optionIcon={<GitHubIcon size={14} />}
              options={[{ value: "", label: "All accounts" }, ...accounts.map((a) => ({ value: a.id, label: a.name }))]}
            />
          </div>
          <div className="w-64">
            <label className="block text-xs text-zinc-500 mb-1">Repository</label>
            <Select
              value={sel.path}
              onChange={sel.setPath}
              searchable
              searchLabel="Search repositories"
              placeholder={loadingRepos ? "Loading repos…" : visibleRepos.length ? "Pick a repository" : "No repositories found"}
              optionIcon={<GitBranch size={14} />}
              options={[
                ...visibleRepos.map((r) => ({
                  value: r.path,
                  label: sel.account ? r.name : `${r.name}  ·  ${r.profile_name}`,
                  ...(r.kind === "folder" ? { note: r.inside ? "inside a repository" : "not a repository yet", icon: <Folder size={14} /> } : {}),
                })),
                ...extraRepos
                  .filter((p) => !visibleRepos.some((r) => r.path === p))
                  .map((p) => ({
                    value: p,
                    label: baseName(p),
                    ...(extraStates[p]?.kind === "no-repo" ? { note: "not a repository yet", icon: <Folder size={14} /> } : {}),
                  })),
              ]}
            />
          </div>
          {(subs.length > 0 || sel.sub) && (
            <div className="w-56">
              <label className="block text-xs text-zinc-500 mb-1">Scope</label>
              <Select
                value={sel.sub}
                onChange={sel.setSub}
                placeholder="This repository"
                optionIcon={<Box size={14} />}
                options={[
                  { value: "", label: "This repository" },
                  ...subs.map((m) => ({ value: m.path, label: subLabel(m) })),
                  ...(sel.sub && !subs.some((m) => m.path === sel.sub) ? [{ value: sel.sub, label: sel.sub }] : []),
                ]}
              />
            </div>
          )}
          <Button
            variant="secondary"
            size="sm"
            onClick={() => setFindOpen(true)}
            title="Look for clones on this computer that no profile's folders hold, and add each to the profile whose GitHub account can reach it"
          >
            <ScanSearch size={14} />
            Find repositories
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={async () => {
              const dir = await pickFolder({ directory: true, multiple: false, title: "Open a repository" });
              if (typeof dir === "string" && dir) {
                setExtraRepos((l) => (l.includes(dir) ? l : [...l, dir]));
                sel.openFolder(dir, gitlinkPaths);
              }
            }}
          >
            Open a folder…
          </Button>
          <Button
            variant="secondary"
            size="sm"
            disabled={!sel.effectivePath}
            onClick={() => openConsole(sel.effectivePath)}
            title="Open the terminal in this repository, or the submodule you are inside"
          >
            <SquareTerminal size={14} />
            Terminal here
          </Button>
        </div>
      </header>

      {(subs.length > 0 || sel.sub) && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-zinc-400 -mt-1">
          {sel.sub ? (
            <>
              <Box size={13} className="text-emerald-400 shrink-0" />
              <span>
                Browsing inside <span className="font-mono text-zinc-200">{sel.sub}</span> — branches, commits, Fetch and every action here belong to this submodule.
              </span>
              <button type="button" onClick={() => sel.setSub("")} className="text-emerald-400 hover:text-emerald-300 cursor-pointer">
                Back to the repository
              </button>
            </>
          ) : (
            <>
              <span className="text-zinc-500">Submodules:</span>
              {subs.map((m) => (
                <button
                  key={m.path}
                  type="button"
                  onClick={() => sel.setSub(m.path)}
                  title={`Work inside this submodule — ${m.summary}`}
                  className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md border border-zinc-700/60 bg-zinc-800/40 hover:border-emerald-500/50 hover:text-zinc-100 cursor-pointer font-mono"
                >
                  {subLabel(m)}
                </button>
              ))}
            </>
          )}
        </div>
      )}

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">{error}</div>
      )}

      {!loadingRepos && repos.length === 0 && !sel.path && (
        <Card className="text-center py-10">
          <GitBranch size={32} className="mx-auto text-zinc-600 mb-3" />
          <h3 className="text-lg font-medium text-zinc-300">No repositories found</h3>
          <p className="text-sm text-zinc-500 mt-1">
            Assign folders to a profile and GitSwitch will list the repos inside them here — or open one folder now.
          </p>
          <div className="mt-4 flex items-center justify-center gap-2">
            <Button variant="secondary" size="sm" onClick={() => navigate("/profile/new")}>
              Set up a profile
            </Button>
          </div>
        </Card>
      )}

      {folder ? (
        <NotARepositoryCard
          entry={folder}
          onMake={() => makeRepository(folder)}
          onOpenRepo={(top) => {
            setExtraRepos((l) => (l.includes(top) ? l : [...l, top]));
            sel.openFolder(top, gitlinkPaths);
          }}
          onEditProfile={(id) => navigate(`/profile/${id}/edit`)}
        />
      ) : (
      <>
      <StatusStrip
        repoPath={sel.effectivePath}
        sub={sel.sub}
        status={status}
        subs={subs}
        loading={loadingStatus}
        busy={busy}
        dirty={dirty}
        pullMode={pullMode}
        pullAutostash={pullAutostash}
        job={job}
        actions={actionsReady ? stripActions : null}
        peek={peek}
        peeking={peeking}
        onPeek={doPeek}
        onPeekDismiss={() => setPeek(null)}
        onPreview={() => setDrawerFor(sel.effectivePath)}
        workspace={sel.sub ? null : workspace}
      />

      {sel.path && !sel.sub && <WorkspaceLine repoPath={sel.path} info={workspace} listeners={listeners} onChanged={setWorkspace} />}

      {sel.effectivePath && (
        <div role="tablist" aria-label="Repository tabs" className="flex flex-wrap items-center gap-1 border-b border-zinc-800">
          {TABS.filter((t) => t !== "submodules" || gitlinks > 0 || sel.sub).map((t) => {
            const active = tab === t;
            const label = t === "changes" ? "Changes" : t === "history" ? "History" : t === "github" ? "GitHub" : "Submodules";
            const Icon = t === "changes" ? FileDiff : t === "history" ? GitGraph : t === "github" ? GitPullRequest : Box;
            const badge =
              t === "changes" && status
                ? status.conflicted_count > 0
                  ? { text: `${status.conflicted_count} conflicted`, cls: "bg-red-500/15 text-red-300" }
                  : changedCount > 0
                    ? { text: String(changedCount), cls: "bg-zinc-700/60 text-zinc-300" }
                    : null
                : t === "submodules" && !sel.sub
                  ? movedCount > 0
                    ? { text: `${subs.length} · ${movedCount} moved`, cls: "bg-sky-500/15 text-sky-300" }
                    : subs.length > 0
                      ? { text: String(subs.length), cls: "bg-zinc-700/60 text-zinc-300" }
                      : null
                  : null;
            return (
              <NavLink
                key={t}
                to={`/repo/${t}`}
                role="tab"
                aria-selected={active}
                className={`flex items-center gap-1.5 px-3 py-2 text-sm border-b-2 -mb-px transition-colors ${
                  active ? "border-emerald-500 text-zinc-100" : "border-transparent text-zinc-400 hover:text-zinc-200"
                }`}
              >
                <Icon size={14} />
                {label}
                {badge && <span className={`text-[10px] px-1.5 py-0.5 rounded ${badge.cls}`}>{badge.text}</span>}
              </NavLink>
            );
          })}
          {/* Decorative: the name is in the picker too, so narrow windows drop it rather than overflow. */}
          <span className="ml-auto hidden md:inline-flex items-center gap-1 text-[11px] text-zinc-600 pr-2 min-w-0 truncate">
            <GitPullRequestArrow size={12} />
            {sel.effectivePath ? baseName(sel.effectivePath) : ""}
          </span>
        </div>
      )}

      {/* All three stay mounted: switching tabs must not drop a running job's
          banner, a half-typed commit message or the history you scrolled to. */}
      <ChangesTab
        repoPath={sel.effectivePath}
        visible={tab === "changes"}
        pullMode={pullMode}
        onPullMode={setPullMode}
        pullLfs={pullLfs}
        onPullLfs={setPullLfs}
        pullAutostash={pullAutostash}
        onPullAutostash={setPullAutostash}
        onOpenRepo={(p) => {
          setExtraRepos((l) => (l.includes(p) ? l : [...l, p]));
          sel.openFolder(p, gitlinkPaths);
        }}
        onStatus={onStatus}
        onActions={onActions}
        onOpenHistory={() => go("history")}
        onAssessAgain={onAssessAgain}
        onGoToConflicts={goToConflicts}
        onPreview={() => setDrawerFor(sel.effectivePath)}
      />
      <HistoryTab
        repo={sel.effectivePath}
        account={sel.account}
        accountEmail={accountEmail}
        accountsReady={!loadingRepos}
        visible={tab === "history"}
        reloadKey={reloadKey}
        refreshKey={refreshKey}
        onOpenChanges={() => go("changes")}
        onRepoChanged={() => actionsRef.current?.refresh()}
        hasSubmodules={gitlinks > 0}
      />
      {/* The scoped repository's submodules: the repository's, or — inside a
          submodule — that submodule's own, which only it can download. */}
      <SubmodulesTab
        repoPath={sel.effectivePath}
        scope={sel.sub}
        visible={tab === "submodules"}
        busy={busy}
        refreshKey={reloadKey}
        // A row is a submodule of what the page shows: at the top a scope, from
        // inside a scope the nested repository itself (openSubOfScope) — never
        // a two-level scope, which the page would drop as unknown.
        onWorkInside={(p) => {
          openSubOfScope(p);
          go("changes");
        }}
        onOpenHistory={(p) => {
          openSubOfScope(p);
          go("history");
        }}
        onInitialise={(paths) => runHere((a) => a.initialise(sel.effectivePath, paths))}
        onRecordPointers={() => runHere((a) => a.recordPointers(sel.effectivePath))}
        onPullInside={(s, source) => runHere((a) => a.pullInside(sel.effectivePath, s, source))}
        onUpdateOne={(p) => runHere((a) => a.updateOne(sel.effectivePath, p))}
        onRecordOne={(p) => runHere((a) => a.recordOne(sel.effectivePath, p))}
        onAlignOne={(p) => runHere((a) => a.alignOne(sel.effectivePath, p))}
        onStartBranch={(p, name) => runHere((a) => a.startBranch(sel.effectivePath, p, name))}
        onPreview={() => {
          // The whole repository is what updates; the drawer is opened for it
          // in the same step, so the scope change does not close it.
          sel.setSub("");
          setDrawerFor(canonicalJoin(sel.path, ""));
        }}
      />

      {sel.effectivePath && (
        <GitHubTab
          repoPath={sel.effectivePath}
          visible={tab === "github"}
          reloadKey={reloadKey}
          // Made again from the Changes tab's Publish dialog, which shows over this tab.
          onCreateAgain={(fullName) => actionsRef.current?.publishAgain?.(fullName)}
        />
      )}

      {/* Only with the status of the repository the drawer is for: right
          after a scope change the page still holds the previous one's. */}
      {drawer && drawerFor && status && normPath(status.path) === drawerFor && (
        <UpdateDrawer
          repoPath={drawerFor}
          status={status}
          busy={busy}
          onClose={() => setDrawerFor(null)}
          onRun={runUpdate}
          onRemedy={remedy}
        />
      )}
      </>
      )}

      <FindReposDialog open={findOpen} onClose={() => setFindOpen(false)} onAdded={() => void loadRepos()} />
    </div>
  );
}
