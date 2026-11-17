import { useEffect, useRef, useState } from "react";
import {
  ScanSearch,
  FolderOpen,
  Loader2,
  CheckCircle2,
  Folder,
  Square,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import type { Profile } from "../types/profile";
import { baseName, isWithin, normPath } from "../lib/paths";
import { usePersistedState } from "../lib/persist";
import * as api from "../lib/api";
import { RefreshButton } from "../components/RefreshButton";

type Phase = "idle" | "scanning" | "resolving" | "checking" | "ready" | "applying";

interface AccountInfo {
  ghAccount: string;
  login: string;
  token: string;
}

interface RepoRow {
  repo: api.ScannedRepo;
  /** login -> permissions (null = that account can't see the repo) */
  access: Record<string, api.RepoPermissions | null>;
  /** logins whose access check ERRORED (rate limit / SSO) — unknown, not "no access" */
  failedLogins: string[];
  currentProfileId: string | null;
  proposedProfileId: string;
  /** Which chip it is under — from the proposal, so a row doesn't jump while its pick is changed. */
  kind: Kind;
}

/** To assign (new, or moving to another profile) · already in a profile · nothing to assign it to. */
type Kind = "assign" | "assigned" | "none";
const kindOf = (current: string | null, proposed: string): Kind =>
  proposed && proposed !== current ? "assign" : current ? "assigned" : "none";
const KIND_LABEL: Record<"all" | Kind, string> = { all: "All", assign: "To assign", assigned: "Already assigned", none: "Not assignable" };

function newScanId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `scan-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }
}

/** The part of `path` below `root`, in the path's own spelling ("" when it is the root). */
function below(path: string, root: string): string | null {
  if (!root || !isWithin(path, root)) return null;
  return path.slice(normPath(root).length).replace(/^[\\/]+/, "");
}

function currentProfileFor(repoPath: string, profiles: Profile[]): Profile | null {
  let best: Profile | null = null;
  let bestLen = -1;
  for (const p of profiles) {
    for (const d of p.directories) {
      const dir = normPath(d);
      if (isWithin(repoPath, dir) && dir.length > bestLen) {
        best = p;
        bestLen = dir.length;
      }
    }
  }
  return best;
}

export function AutoAssign() {
  const toast = useToast();
  const [root, setRoot] = usePersistedState("autoassign.root", "");
  const [phase, setPhase] = useState<Phase>("idle");
  const [statusLine, setStatusLine] = useState("");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [profileLogins, setProfileLogins] = useState<Record<string, string | null>>({});
  const [loginSources, setLoginSources] = useState<Record<string, "key" | "saved" | null>>({});
  const [rows, setRows] = useState<RepoRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  /** Where the walk is, while it walks. */
  const [progress, setProgress] = useState<api.FindProgress | null>(null);
  /** The folder the shown results come from, and how many folders were taken up. */
  const [scanned, setScanned] = useState<{ root: string; folders: number } | null>(null);
  const [stopped, setStopped] = useState(false);
  const [filter, setFilter] = useState<"all" | Kind>("all");
  /** The scan running now; whatever an earlier one (stopped, or the page left) answers is dropped. */
  const current = useRef<string | null>(null);

  // Leaving the page stops the walk.
  useEffect(
    () => () => {
      const id = current.current;
      current.current = null;
      if (id) api.findReposCancel(id).catch(() => {});
    },
    []
  );

  async function browseRoot() {
    const dir = await open({ directory: true, multiple: false, title: "Folder to scan for repos" });
    if (typeof dir === "string") setRoot(dir);
  }

  async function runScan() {
    if (!root) {
      setError("Choose a folder to scan first.");
      return;
    }
    setError(null);
    setRows([]);
    setScanned(null);
    setStopped(false);
    setProgress(null);
    setFilter("all");
    const id = newScanId();
    current.current = id;
    const live = () => current.current === id;
    try {
      // 1. Find repos — watched, so the page says where it is, and can stop.
      setPhase("scanning");
      setStatusLine(`Searching ${root}`);
      const poll = window.setInterval(() => {
        api
          .findReposProgress(id)
          .then((p) => {
            if (live() && p && !Array.isArray(p)) setProgress(p);
          })
          .catch(() => {});
      }, 300);
      let scan: api.TrackedScan;
      try {
        scan = await api.scanReposTracked(id, root);
      } finally {
        window.clearInterval(poll);
      }
      if (!live()) return;
      setProgress(null);
      if (scan.cancelled) {
        setPhase("idle");
        setStatusLine("");
        setStopped(true);
        return;
      }
      setScanned({ root: scan.root, folders: scan.folders });
      const repos = scan.repos;
      if (repos.length === 0) {
        setPhase("ready");
        setStatusLine("");
        toast.success("No git repositories found in that folder");
        return;
      }

      // 2. Which GitHub account does each profile's SSH key belong to?
      setPhase("resolving");
      setStatusLine("Resolving which account each profile's SSH key belongs to…");
      const profs = await api.getProfiles();
      if (!live()) return;
      setProfiles(profs);
      // The key first — it is who git authenticates as; without a key the
      // check can use (none, or one locked by a passphrase), the GitHub
      // account the profile says it pushes as.
      const logins: Record<string, string | null> = {};
      const sources: Record<string, "key" | "saved" | null> = {};
      await Promise.all(
        profs.map(async (p) => {
          let byKey: string | null = null;
          if (p.ssh_key_path) {
            try {
              byKey = await api.resolveKeyAccount(p.ssh_key_path);
            } catch {
              byKey = null;
            }
          }
          const saved = (p.github_login ?? "").trim().replace(/^@/, "") || null;
          logins[p.id] = byKey ?? saved;
          sources[p.id] = byKey ? "key" : saved ? "saved" : null;
        })
      );
      if (!live()) return;
      setProfileLogins(logins);
      setLoginSources(sources);

      // 3. Collect gh accounts + tokens
      setStatusLine("Loading GitHub CLI accounts…");
      let accounts: AccountInfo[] = [];
      try {
        const names = await api.ghListAccounts();
        accounts = (
          await Promise.all(
            names.map(async (ghAccount) => {
              try {
                const token = await api.ghGetToken(ghAccount);
                const user = await api.verifyGithubToken(token);
                return { ghAccount, login: user.login, token };
              } catch {
                return null;
              }
            })
          )
        ).filter((a): a is AccountInfo => a !== null);
      } catch {
        accounts = [];
      }
      if (!live()) return;
      if (accounts.length === 0) {
        throw new Error(
          "No GitHub CLI accounts available — sign in with `gh auth login` so access can be checked."
        );
      }

      // 4. Check every repo × account (dedup identical owner/name pairs).
      //    Errors (rate limit / SSO) are tracked separately from "no access" —
      //    conflating them would propose wrong accounts.
      setPhase("checking");
      const githubRepos = repos.filter((r) => r.owner && r.name);
      const uniqueKeys = Array.from(new Set(githubRepos.map((r) => `${r.owner}/${r.name}`)));
      const accessByKey: Record<string, Record<string, api.RepoPermissions | null>> = {};
      const failedByKey: Record<string, string[]> = {};
      let done = 0;
      for (const key of uniqueKeys) {
        if (!live()) return;
        const [owner, name] = key.split("/");
        const perKey: Record<string, api.RepoPermissions | null> = {};
        const failed: string[] = [];
        await Promise.all(
          accounts.map(async (a) => {
            try {
              perKey[a.login] = await api.checkRepoAccess(a.token, owner, name);
            } catch {
              failed.push(a.login);
            }
          })
        );
        accessByKey[key] = perKey;
        failedByKey[key] = failed;
        done++;
        setStatusLine(`Checking repo access… ${done}/${uniqueKeys.length}`);
      }

      // 5. Propose an assignment per repo
      const newRows: RepoRow[] = repos.map((repo) => {
        const key = `${repo.owner}/${repo.name}`;
        const access = repo.owner ? accessByKey[key] ?? {} : {};
        const failedLogins = repo.owner ? failedByKey[key] ?? [] : [];
        const current = currentProfileFor(repo.path, profs);

        // Any failed check = incomplete information: never auto-propose, let
        // the user decide manually.
        let proposed = "";
        if (failedLogins.length === 0) {
          const qualifies = (p: Profile, wantPush: boolean) => {
            const login = logins[p.id];
            if (!login) return false;
            const perms = access[login];
            return !!perms && (wantPush ? perms.push : perms.pull);
          };
          // Prefer PUSH access over read; within a tier, prefer the repo's
          // CURRENT profile so correctly-assigned repos are never churned.
          for (const wantPush of [true, false]) {
            if (proposed) break;
            if (current && qualifies(current, wantPush)) {
              proposed = current.id;
              break;
            }
            for (const p of profs) {
              if (qualifies(p, wantPush)) {
                proposed = p.id;
                break;
              }
            }
          }
        }

        return {
          repo,
          access,
          failedLogins,
          currentProfileId: current?.id ?? null,
          proposedProfileId: proposed,
          kind: kindOf(current?.id ?? null, proposed),
        };
      });

      if (!live()) return;
      setRows(newRows);
      setPhase("ready");
      setStatusLine("");
    } catch (e) {
      if (!live()) return;
      setError(String(e));
      setPhase("idle");
      setStatusLine("");
      setProgress(null);
    }
  }

  /** Stop the walk (and drop whatever it, or the checks after it, answer later). */
  function stop() {
    const id = current.current;
    current.current = null;
    if (id) api.findReposCancel(id).catch(() => {});
    setPhase("idle");
    setStatusLine("");
    setProgress(null);
    setStopped(true);
  }

  function setRowProfile(path: string, profileId: string) {
    setRows((rs) =>
      rs.map((r) => (r.repo.path === path ? { ...r, proposedProfileId: profileId } : r))
    );
  }

  const changes = rows.filter(
    (r) => r.proposedProfileId && r.proposedProfileId !== r.currentProfileId
  );

  async function applyAll() {
    if (changes.length === 0) return;
    setPhase("applying");
    setError(null);
    try {
      // Group new folders by target profile; dedupe in the backend strips them
      // from any other profile automatically.
      const byProfile = new Map<string, string[]>();
      for (const c of changes) {
        const list = byProfile.get(c.proposedProfileId) ?? [];
        list.push(c.repo.path);
        byProfile.set(c.proposedProfileId, list);
      }
      // CRITICAL: re-fetch the store before AND between updates. Merging from a
      // stale snapshot would re-add directories that an earlier update in this
      // same batch just moved away (backend dedupe strips them from the other
      // profile — a stale merge would silently undo that).
      let profs = await api.getProfiles();
      for (const [profileId, dirs] of byProfile) {
        const prof = profs.find((p) => p.id === profileId);
        if (!prof) continue;
        const merged = Array.from(new Set([...prof.directories, ...dirs]));
        await api.updateProfile({ id: profileId, directories: merged });
        profs = await api.getProfiles();
      }
      const nameOf = (pid: string | null | undefined) => (pid ? profs.find((p) => p.id === pid)?.name : undefined);
      toast.success(
        `Assigned ${changes.length} repo${changes.length === 1 ? "" : "s"} to the right account`,
        changes.map((c) => {
          const was = nameOf(c.currentProfileId);
          return `${baseName(c.repo.path)} → ${nameOf(c.proposedProfileId) ?? "its profile"}${was ? ` (was ${was})` : ""}`;
        })
      );
      // Refresh state so rows show as aligned.
      setProfiles(profs);
      setRows((rs) =>
        rs.map((r) => {
          const now = currentProfileFor(r.repo.path, profs)?.id ?? null;
          return { ...r, currentProfileId: now, kind: kindOf(now, r.proposedProfileId) };
        })
      );
      setPhase("ready");
    } catch (e) {
      setError(String(e));
      setPhase("ready");
    }
  }

  const busy =
    phase === "scanning" ||
    phase === "resolving" ||
    phase === "checking" ||
    phase === "applying";

  const profileLabel = (id: string | null) => {
    const p = profiles.find((x) => x.id === id);
    if (!p) return "a profile";
    return profileLogins[p.id] ? `${p.name} (${profileLogins[p.id]})` : p.name;
  };
  const shownRoot = scanned?.root ?? root;
  /** "client/widgets" for a repository under the folder searched; the whole path otherwise. */
  const where = (path: string) => {
    const rest = below(path, shownRoot);
    return rest === null ? path : rest || ".";
  };
  const count = (k: "all" | Kind) => (k === "all" ? rows.length : rows.filter((r) => r.kind === k).length);
  const listed = filter === "all" ? rows : rows.filter((r) => r.kind === filter);

  const progressLine = (() => {
    if (phase !== "scanning") return statusLine;
    if (progress?.phase === "reading") return `Reading ${progress.total} repositor${progress.total === 1 ? "y" : "ies"} found in ${root} · ${progress.checked}/${progress.total}`;
    const n = progress?.folders ?? 0;
    const f = progress?.found ?? 0;
    return `Searching ${root}${n ? ` · ${n.toLocaleString()} folder${n === 1 ? "" : "s"}` : ""}${f ? ` · ${f} found` : ""}`;
  })();

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Auto Assign</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Scan a folder for repos, detect which of your GitHub accounts has
            access to each, and map them to the right profile automatically.
          </p>
        </div>
        <RefreshButton onClick={runScan} busy={busy} disabled={!root} title={root ? "Scan the folder again" : "Choose a folder to scan first"} />
      </div>

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      <Card>
        <div className="flex gap-2 items-center">
          <div className="flex-1 px-3 py-2 rounded-lg bg-zinc-800 border border-zinc-700 text-sm font-mono truncate text-zinc-300">
            {root || <span className="text-zinc-500">Choose a folder to scan…</span>}
          </div>
          <Button type="button" variant="secondary" onClick={browseRoot} disabled={busy}>
            <FolderOpen size={16} />
            Browse
          </Button>
          <Button onClick={runScan} disabled={busy || !root}>
            {busy ? (
              <>
                <Loader2 size={16} className="animate-spin" />
                Working…
              </>
            ) : (
              <>
                <ScanSearch size={16} />
                Scan
              </>
            )}
          </Button>
        </div>
        {busy && progressLine && (
          <div className="mt-3 space-y-1" data-autoassign-progress>
            <div className="flex items-center justify-between gap-3">
              <p className="text-xs text-zinc-500 flex items-center gap-2 min-w-0" role="status">
                <Loader2 size={12} className="animate-spin text-emerald-400 shrink-0" />
                <span className="truncate" title={progressLine}>
                  {progressLine}
                </span>
              </p>
              {phase !== "applying" && (
                <Button size="sm" variant="ghost" onClick={stop}>
                  <Square size={12} />
                  Stop
                </Button>
              )}
            </div>
            {phase === "scanning" && progress?.place && (
              <p className="text-xs text-zinc-600 font-mono truncate pl-5" title={progress.place} data-autoassign-place>
                in {where(progress.place)}
              </p>
            )}
          </div>
        )}
        {stopped && !busy && <p className="text-xs text-zinc-400 mt-3">Stopped. Nothing was changed — Scan to search again.</p>}
      </Card>

      {rows.length > 0 && (
        <Card>
          <div className="flex items-center justify-between mb-3 gap-3 flex-wrap">
            <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider min-w-0" data-autoassign-found={rows.length}>
              {rows.length} repo{rows.length === 1 ? "" : "s"} found
              <span className="normal-case tracking-normal font-normal text-zinc-400">
                {" "}
                in <span className="font-mono text-zinc-300 break-all">{shownRoot}</span>
              </span>
            </h2>
            <Button
              size="sm"
              onClick={applyAll}
              disabled={phase === "applying" || changes.length === 0}
            >
              {phase === "applying" ? (
                <>
                  <Loader2 size={14} className="animate-spin" />
                  Applying…
                </>
              ) : (
                `Apply ${changes.length} assignment${changes.length === 1 ? "" : "s"}`
              )}
            </Button>
          </div>

          {scanned && (
            <p className="text-xs text-zinc-500 -mt-1 mb-3">
              Searched {scanned.folders.toLocaleString()} folder{scanned.folders === 1 ? "" : "s"}, up to five levels deep. Hidden folders, node_modules, target, dist,
              build and Library are passed over, and a repository's own folders are not searched.
            </p>
          )}

          <div className="inline-flex rounded-lg bg-zinc-800/80 p-0.5 flex-wrap mb-3" role="group" aria-label="Show">
            {(["all", "assign", "assigned", "none"] as const).map((k) => (
              <button
                key={k}
                type="button"
                data-autoassign-chip={k}
                aria-pressed={filter === k}
                onClick={() => setFilter(k)}
                className={`px-3 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer ${
                  filter === k ? "bg-zinc-700 text-emerald-400" : "text-zinc-400 hover:text-zinc-200"
                }`}
              >
                {KIND_LABEL[k]} <span className="text-zinc-500">{count(k)}</span>
              </button>
            ))}
          </div>
          {listed.length === 0 && <p className="text-xs text-zinc-500 mb-2">None here.</p>}

          <div className="space-y-1.5">
            {listed.map((r) => {
              const aligned =
                r.proposedProfileId !== "" && r.proposedProfileId === r.currentProfileId;
              const accessEntries = Object.entries(r.access).filter(([, p]) => p !== null);
              return (
                <div
                  key={r.repo.path}
                  data-autoassign-row={r.repo.path}
                  className="flex items-center gap-3 px-3 py-2.5 rounded-lg bg-zinc-800/50 border border-zinc-700/50 min-w-0 flex-wrap lg:flex-nowrap"
                >
                  <Folder size={15} className="text-zinc-500 shrink-0" />
                  <div className="min-w-0 flex-1">
                    <p className="text-sm text-zinc-200 font-medium truncate">
                      {baseName(r.repo.path)}
                    </p>
                    <p className="text-xs text-zinc-500 font-mono truncate">
                      {r.repo.owner
                        ? `${r.repo.owner}/${r.repo.name}`
                        : r.repo.remote_url || "no remote"}
                    </p>
                    <p className="text-xs font-mono truncate" title={r.repo.path} data-autoassign-path>
                      {(() => {
                        // The whole path, the folder searched dimmed: where it is, at a glance.
                        const rest = below(r.repo.path, shownRoot);
                        if (!rest) return <span className="text-zinc-500">{r.repo.path}</span>;
                        return (
                          <>
                            <span className="text-zinc-600">{r.repo.path.slice(0, r.repo.path.length - rest.length)}</span>
                            <span className="text-zinc-400">{rest}</span>
                          </>
                        );
                      })()}
                    </p>
                    {r.currentProfileId && (
                      <p className="mt-1">
                        {aligned ? (
                          <span className="inline-flex items-center px-2 py-0.5 rounded-full text-[11px] bg-emerald-500/15 text-emerald-300 border border-emerald-500/30" data-autoassign-status="aligned">
                            Already in {profileLabel(r.currentProfileId)}
                          </span>
                        ) : r.proposedProfileId && r.proposedProfileId !== r.currentProfileId ? (
                          <span className="inline-flex items-center px-2 py-0.5 rounded-full text-[11px] bg-amber-500/15 text-amber-300 border border-amber-500/30" data-autoassign-status="move">
                            In {profileLabel(r.currentProfileId)} now → Apply moves it to {profileLabel(r.proposedProfileId)}
                          </span>
                        ) : (
                          <span className="inline-flex items-center px-2 py-0.5 rounded-full text-[11px] bg-zinc-700/50 text-zinc-300 border border-zinc-600/60" data-autoassign-status="in">
                            In {profileLabel(r.currentProfileId)}
                          </span>
                        )}
                      </p>
                    )}
                  </div>

                  {/* Below 1024 px the reasons get a line of their own rather than disappearing. */}
                  <div className="flex flex-wrap items-center gap-1 basis-full order-last lg:order-none lg:basis-auto lg:flex-nowrap lg:shrink-0 lg:max-w-[38%] lg:overflow-hidden">
                    {r.repo.owner ? (
                      <>
                        {accessEntries.map(([login, perms]) => (
                          <Badge
                            key={login}
                            variant={perms!.push ? "success" : "default"}
                          >
                            {perms!.push ? "push" : "read"} · {login}
                          </Badge>
                        ))}
                        {r.failedLogins.length > 0 && (
                          <Badge variant="warning">
                            check failed · {r.failedLogins.join(", ")}
                          </Badge>
                        )}
                        {accessEntries.length === 0 && r.failedLogins.length === 0 && (
                          <Badge variant="warning">no account has access</Badge>
                        )}
                      </>
                    ) : (
                      <Badge variant="default">not GitHub</Badge>
                    )}
                  </div>

                  <div className="w-full sm:w-44 shrink-0">
                    <Select
                      value={r.proposedProfileId}
                      onChange={(v) => setRowProfile(r.repo.path, v)}
                      placeholder="Don't assign"
                      options={[
                        { value: "", label: "Don't assign" },
                        ...profiles.map((p) => ({
                          value: p.id,
                          label: profileLogins[p.id]
                            ? `${p.name} (${profileLogins[p.id]}${loginSources[p.id] === "saved" ? " · saved account" : ""})`
                            : p.name,
                        })),
                      ]}
                    />
                  </div>

                  {aligned && (
                    <CheckCircle2
                      size={16}
                      className="text-emerald-400 shrink-0"
                      aria-label="already assigned correctly"
                    />
                  )}
                </div>
              );
            })}
          </div>
          <p className="text-xs text-zinc-600 mt-3">
            "push · account" means that account can push to the repo — repos are
            proposed to the profile whose SSH key — or, without a usable key, whose
            saved GitHub account — has push access. A folder moves to exactly one
            profile; conflicts are cleaned up automatically.
          </p>
        </Card>
      )}
    </div>
  );
}
