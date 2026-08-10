import { useState } from "react";
import {
  ScanSearch,
  FolderOpen,
  Loader2,
  CheckCircle2,
  Folder,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import type { Profile } from "../types/profile";
import * as api from "../lib/api";

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
}

function currentProfileFor(repoPath: string, profiles: Profile[]): Profile | null {
  let best: Profile | null = null;
  let bestLen = -1;
  for (const p of profiles) {
    for (const d of p.directories) {
      const dir = d.replace(/\/+$/, "");
      if ((repoPath === dir || repoPath.startsWith(dir + "/")) && dir.length > bestLen) {
        best = p;
        bestLen = dir.length;
      }
    }
  }
  return best;
}

export function AutoAssign() {
  const toast = useToast();
  const [root, setRoot] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [statusLine, setStatusLine] = useState("");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [profileLogins, setProfileLogins] = useState<Record<string, string | null>>({});
  const [rows, setRows] = useState<RepoRow[]>([]);
  const [error, setError] = useState<string | null>(null);

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
    try {
      // 1. Find repos
      setPhase("scanning");
      setStatusLine("Scanning for git repositories…");
      const repos = await api.scanRepos(root);
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
      setProfiles(profs);
      const logins: Record<string, string | null> = {};
      await Promise.all(
        profs.map(async (p) => {
          if (!p.ssh_key_path) {
            logins[p.id] = null;
            return;
          }
          try {
            logins[p.id] = await api.resolveKeyAccount(p.ssh_key_path);
          } catch {
            logins[p.id] = null;
          }
        })
      );
      setProfileLogins(logins);

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
        };
      });

      setRows(newRows);
      setPhase("ready");
      setStatusLine("");
    } catch (e) {
      setError(String(e));
      setPhase("idle");
      setStatusLine("");
    }
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
      toast.success(
        `Assigned ${changes.length} repo${changes.length === 1 ? "" : "s"} to the right account`
      );
      // Refresh state so rows show as aligned.
      setProfiles(profs);
      setRows((rs) =>
        rs.map((r) => ({
          ...r,
          currentProfileId: currentProfileFor(r.repo.path, profs)?.id ?? null,
        }))
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

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">Auto Assign</h1>
        <p className="text-sm text-zinc-400 mt-1">
          Scan a folder for repos, detect which of your GitHub accounts has
          access to each, and map them to the right profile automatically.
        </p>
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
        {busy && statusLine && (
          <p className="text-xs text-zinc-500 mt-3 flex items-center gap-2">
            <Loader2 size={12} className="animate-spin text-emerald-400" />
            {statusLine}
          </p>
        )}
      </Card>

      {rows.length > 0 && (
        <Card>
          <div className="flex items-center justify-between mb-3">
            <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider">
              {rows.length} repo{rows.length === 1 ? "" : "s"} found
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

          <div className="space-y-1.5">
            {rows.map((r) => {
              const aligned =
                r.proposedProfileId !== "" && r.proposedProfileId === r.currentProfileId;
              const accessEntries = Object.entries(r.access).filter(([, p]) => p !== null);
              return (
                <div
                  key={r.repo.path}
                  className="flex items-center gap-3 px-3 py-2.5 rounded-lg bg-zinc-800/50 border border-zinc-700/50"
                >
                  <Folder size={15} className="text-zinc-500 shrink-0" />
                  <div className="min-w-0 flex-1">
                    <p className="text-sm text-zinc-200 font-medium truncate">
                      {r.repo.path.split("/").pop()}
                    </p>
                    <p className="text-xs text-zinc-500 font-mono truncate">
                      {r.repo.owner
                        ? `${r.repo.owner}/${r.repo.name}`
                        : r.repo.remote_url || "no remote"}
                    </p>
                  </div>

                  <div className="hidden md:flex items-center gap-1 shrink-0">
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

                  <div className="w-44 shrink-0">
                    <Select
                      value={r.proposedProfileId}
                      onChange={(v) => setRowProfile(r.repo.path, v)}
                      placeholder="Don't assign"
                      options={[
                        { value: "", label: "Don't assign" },
                        ...profiles.map((p) => ({
                          value: p.id,
                          label: profileLogins[p.id]
                            ? `${p.name} (${profileLogins[p.id]})`
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
            proposed to the profile whose SSH key belongs to an account with push
            access. A folder moves to exactly one profile; conflicts are cleaned
            up automatically.
          </p>
        </Card>
      )}
    </div>
  );
}
