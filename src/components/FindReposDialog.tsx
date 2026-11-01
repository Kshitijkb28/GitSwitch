import { useEffect, useRef, useState, type ReactNode } from "react";
import { CheckCircle2, Folder, FolderPlus, Link2, Loader2, ScanSearch, Square, Undo2, X } from "lucide-react";
import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import { Modal } from "./Modal";
import { Button } from "./Button";
import { Badge } from "./Badge";
import { Select } from "./Select";
import { Checkbox } from "./Checkbox";
import { useToast } from "./Toast";
import type { Profile } from "../types/profile";
import * as api from "../lib/api";
import { baseName, isWithin, normPath } from "../lib/paths";
import { readPersisted, usePersistedState } from "../lib/persist";
import { accessKeys, loginOf, propose, pushKey, type ProfileLogin, type Proposal } from "../lib/findRepos";

type Phase = "intro" | "looking" | "checking" | "ready" | "adding";

interface Row {
  repo: api.FoundRepo;
  proposal: Proposal;
  /** The profile picked; "" = don't add. */
  choice: string;
}

/**
 * The last repositories added and how to take them off again — kept while the
 * app runs, so Undo is still there when the dialog is closed and opened again.
 * A new search starts a new batch.
 */
let lastAdded: { items: api.FolderAdded[]; labels: Record<string, string> } | null = null;

function newScanId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `find-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }
}

function plural(n: number, one: string, many: string) {
  return `${n} ${n === 1 ? one : many}`;
}

const clean = (e: unknown) => String(e).replace(/^(Error: )?(Command execution error: )?/, "");

const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

/** Which GitHub account each profile is: the one its SSH key signs in as, else the one it names. */
async function profileLogins(profs: Profile[]): Promise<Record<string, ProfileLogin>> {
  const out: Record<string, ProfileLogin> = {};
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
      const display = byKey ?? saved;
      out[p.id] = { login: display ? display.toLowerCase() : null, display, source: byKey ? "key" : saved ? "saved" : null };
    })
  );
  return out;
}

/**
 * Find repositories: the clones on this computer that no profile's folders
 * hold — so the Repository page does not list them — each matched to the
 * profile whose GitHub account can reach it, and added to it on one click
 * (or straight away, when the match is clear and the switch is on).
 */
export function FindReposDialog({ open, onClose, onAdded }: { open: boolean; onClose: () => void; onAdded: () => void }) {
  const toast = useToast();
  const [places, setPlaces] = usePersistedState<string[]>("findrepos.places", []);
  const [autoAdd, setAutoAdd] = usePersistedState("findrepos.autoAdd", false);
  const [searched, setSearched] = usePersistedState("findrepos.searched", false);
  const [phase, setPhase] = useState<Phase>("intro");
  const [found, setFound] = useState<api.FoundClones | null>(null);
  const [rows, setRows] = useState<Row[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [logins, setLogins] = useState<Record<string, ProfileLogin>>({});
  const [matrix, setMatrix] = useState<api.AccessMatrix | null>(null);
  const [progress, setProgress] = useState<api.FindProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [stopped, setStopped] = useState(false);
  const [added, setAdded] = useState(lastAdded);
  const [showPlaces, setShowPlaces] = useState(false);
  /** The search running now; anything an earlier one (stopped, or the dialog closed) answers is dropped. */
  const current = useRef<string | null>(null);

  const remembered = [
    { path: readPersisted<string>("sparse.parentDir", ""), label: "the Clone page's folder" },
    { path: readPersisted<string>("autoassign.root", ""), label: "the Auto Assign folder" },
  ].filter((r) => r.path.trim());

  function extraRoots(list: string[] = places): string[] {
    const out: string[] = [];
    for (const p of [...list, ...remembered.map((r) => r.path)]) {
      const t = p.trim();
      if (t && !out.some((o) => normPath(o) === normPath(t))) out.push(t);
    }
    return out;
  }

  const home = found?.home ?? null;
  const short = (p: string) => (home && isWithin(p, home) ? `~${normPath(p).slice(normPath(home).length) || ""}` : p);
  const profileName = (profs: Profile[], id: string) => profs.find((p) => p.id === id)?.name ?? "a profile";

  async function add(list: Row[], profs: Profile[]): Promise<Set<string>> {
    if (list.length === 0) return new Set();
    setPhase("adding");
    try {
      const res = await api.profileFoldersAdd(list.map((r) => ({ profile_id: r.choice, path: r.repo.path })));
      if (res.added.length) {
        const labels = { ...(lastAdded?.labels ?? {}) };
        for (const a of res.added) labels[a.path] = `${baseName(a.path)} · ${profileName(profs, a.profile_id)}`;
        lastAdded = { items: [...(lastAdded?.items ?? []), ...res.added], labels };
        setAdded(lastAdded);
        onAdded();
        toast.success(`Added ${plural(res.added.length, "repository", "repositories")} to ${res.added.length === 1 ? "its profile" : "their profiles"}`);
      }
      if (res.failed.length) setError(res.failed.map((f) => `${baseName(f.path)}: ${f.why}`).join("\n"));
      const failed = new Set(res.failed.map((f) => f.path));
      return new Set(list.map((r) => r.repo.path).filter((p) => !failed.has(p)));
    } catch (e) {
      setError(clean(e));
      return new Set();
    } finally {
      setPhase("ready");
    }
  }

  async function run(list: string[] = places) {
    const id = newScanId();
    current.current = id;
    const live = () => current.current === id;
    setError(null);
    setStopped(false);
    setFound(null);
    setRows([]);
    setMatrix(null);
    setProgress(null);
    setSearched(true);
    lastAdded = null;
    setAdded(null);
    setPhase("looking");
    const poll = window.setInterval(() => {
      api
        .findReposProgress(id)
        .then((p) => live() && p && setProgress(p))
        .catch(() => {});
    }, 400);
    try {
      const f = await api.findUnlistedRepos(id, extraRoots(list));
      if (!live()) return;
      setFound(f);
      if (f.cancelled || f.repos.length === 0) {
        setPhase("ready");
        return;
      }
      setPhase("checking");
      const profs = await api.getProfiles();
      const ls = await profileLogins(profs);
      if (!live()) return;
      const keys = accessKeys(f.repos);
      let m: api.AccessMatrix | null = null;
      if (keys.length) {
        try {
          m = await api.repoAccessMatrix(id, keys);
        } catch (e) {
          setError(`GitHub could not be asked: ${clean(e)}`);
        }
      }
      if (!live()) return;
      let proposals = f.repos.map((r) => propose(r, profs, ls, m));
      // An organisation's repository nobody's login owns: the profile its other
      // clones here use is preselected (never added unasked). Read only when needed.
      if (proposals.some((p, i) => !p.profileId && f.repos[i].owner)) {
        try {
          const hints = (await api.localCloneIndex()).owner_profiles;
          if (!live()) return;
          proposals = f.repos.map((r) => propose(r, profs, ls, m, hints));
        } catch {
          /* no hints: the rows stay as they are */
        }
      }
      const all: Row[] = f.repos.map((r, i) => ({ repo: r, proposal: proposals[i], choice: proposals[i].profileId }));
      setProfiles(profs);
      setLogins(ls);
      setMatrix(m);
      setRows(all);
      if (autoAdd) {
        const clear = all.filter((r) => r.proposal.clear && r.choice);
        if (clear.length && live()) {
          const done = await add(clear, profs);
          setRows(all.filter((r) => !done.has(r.repo.path)));
          return;
        }
      }
      setPhase("ready");
    } catch (e) {
      if (live()) {
        setError(clean(e));
        setPhase("ready");
      }
    } finally {
      window.clearInterval(poll);
      if (live()) setProgress(null);
    }
  }

  function abandon() {
    const id = current.current;
    current.current = null;
    if (id && (phase === "looking" || phase === "checking")) api.findReposCancel(id).catch(() => {});
  }

  function stop() {
    abandon();
    setStopped(true);
    setProgress(null);
    setPhase("ready");
  }

  function close() {
    // Whatever is still running is stopped, and nothing it finds is added.
    abandon();
    onClose();
  }

  async function undo() {
    if (!lastAdded) return;
    setPhase("adding");
    try {
      const removed = await api.profileFoldersRemove(lastAdded.items);
      lastAdded = null;
      setAdded(null);
      onAdded();
      toast.success(`Took ${plural(removed.length, "repository", "repositories")} off again`);
    } catch (e) {
      setError(clean(e));
    } finally {
      setPhase("ready");
    }
  }

  useEffect(() => {
    if (!open) return;
    setError(null);
    setShowPlaces(false);
    if (lastAdded) {
      // Opened again after adding: what was added, with Undo — no new search yet.
      setAdded(lastAdded);
      setFound(null);
      setRows([]);
      setPhase("ready");
    } else if (searched) {
      void run();
    } else {
      setPhase("intro");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function lookIn(path?: string) {
    let dir = path;
    if (!dir) {
      const picked = await pickFolder({ directory: true, multiple: false, title: "Also look in this folder" });
      if (typeof picked !== "string" || !picked) return;
      dir = picked;
    }
    const next = places.some((p) => normPath(p) === normPath(dir!)) ? places : [...places, dir];
    setPlaces(next);
    if (phase !== "intro") void run(next);
  }

  function forget(path: string) {
    setPlaces(places.filter((p) => normPath(p) !== normPath(path)));
  }

  const busy = phase === "looking" || phase === "checking" || phase === "adding";
  const chosen = rows.filter((r) => r.choice);
  const loginShown = (l: string) => matrix?.accounts.find((a) => a.login.toLowerCase() === l)?.login ?? l;

  function badges(r: api.FoundRepo) {
    if (!matrix || r.gitdir_elsewhere || r.commits_as.via === "folder") return null;
    const rk = r.owner && r.name ? `${r.owner}/${r.name}`.toLowerCase() : "";
    const pk = pushKey(r);
    const asked = new Set([...Object.keys(matrix.access[rk] ?? {}), ...Object.keys(matrix.access[pk] ?? {})]);
    const out: ReactNode[] = [];
    for (const l of asked) {
      const push = matrix.access[pk]?.[l];
      const read = matrix.access[rk]?.[l];
      if (push?.state === "unknown" || read?.state === "unknown") {
        out.push(
          <Badge key={l} variant="warning">
            check failed · {loginShown(l)}
          </Badge>
        );
      } else if (push?.state === "visible" && push.push) {
        out.push(
          <Badge key={l} variant="success">
            push · {loginShown(l)}
          </Badge>
        );
      } else if (read?.state === "visible") {
        out.push(<Badge key={l}>read · {loginShown(l)}</Badge>);
      }
    }
    return out.length ? <div className="flex flex-wrap gap-1 mt-1">{out}</div> : null;
  }

  function commitsLine(r: Row) {
    const ca = r.repo.commits_as;
    const now =
      ca.via === "local" ? "its own email" : ca.profile_id ? profileName(profiles, ca.profile_id) : ca.via === "none" ? "no profile" : "the default profile";
    const after = r.choice && r.choice !== ca.profile_id && ca.via !== "local" ? ` · after adding: ${profileName(profiles, r.choice)}` : "";
    return `Commits as ${now}${after}`;
  }

  function options(r: Row) {
    const label = (p: Profile) => {
      const l = logins[p.id]?.display ?? loginOf(logins, p);
      return l ? `${p.name} (${l})` : p.name;
    };
    const offered = r.proposal.only && r.proposal.profileId ? profiles.filter((p) => p.id === r.proposal.profileId) : profiles;
    return [{ value: "", label: "Don't add" }, ...offered.map((p) => ({ value: p.id, label: label(p) }))];
  }

  const statusLine = (() => {
    if (phase === "looking") {
      const n = progress?.folders ?? 0;
      return `Looking for repositories${n ? ` · ${n.toLocaleString()} folders` : ""}${progress?.place ? ` · ${short(progress.place)}` : ""}`;
    }
    if (phase === "checking") {
      return progress?.phase === "checking" && progress.total
        ? `Asking GitHub which account can reach each one · ${progress.checked}/${progress.total}`
        : "Checking which GitHub account each profile uses…";
    }
    if (phase === "adding") return "Saving…";
    return "";
  })();

  const nothingNew = phase === "ready" && found && !found.cancelled && found.repos.length === 0 && !stopped;

  return (
    <Modal open={open} onClose={close} title="Find repositories" size="lg">
      <div className="flex flex-col max-h-[calc(100vh-9rem)] text-sm">
        <div className="space-y-4 overflow-y-auto min-h-0 -mr-2 pr-2">
          <p className="text-zinc-400">
            Clones on this computer that no profile's folders hold are missing from the Repository page, and commit as the default profile. This finds them and
            adds each to the profile whose GitHub account can reach it.
          </p>

          <div className="rounded-lg border border-zinc-700/60 bg-zinc-800/40 px-3 py-2.5 space-y-2">
            <div className="flex items-center justify-between gap-2 flex-wrap">
              <button
                type="button"
                onClick={() => setShowPlaces((v) => !v)}
                className="text-zinc-300 hover:text-zinc-100 cursor-pointer text-left"
                aria-expanded={showPlaces}
              >
                Looking in your home folder{extraRoots().length ? ` and ${plural(extraRoots().length, "more place", "more places")}` : ""}
                <span className="text-zinc-500"> · {showPlaces ? "hide" : "show"}</span>
              </button>
              <Button size="sm" variant="secondary" onClick={() => void lookIn()} disabled={busy}>
                <FolderPlus size={14} />
                Also look in…
              </Button>
            </div>
            {showPlaces && (
              <ul className="space-y-1 text-xs">
                <li className="text-zinc-400">Your home folder, five folders deep</li>
                {remembered.map((r) => (
                  <li key={r.path} className="text-zinc-400 truncate" title={r.path}>
                    <span className="font-mono text-zinc-300">{short(r.path)}</span> · {r.label}
                  </li>
                ))}
                {places.map((p) => (
                  <li key={p} className="flex items-center gap-2 min-w-0">
                    <span className="font-mono text-zinc-300 truncate" title={p}>
                      {short(p)}
                    </span>
                    <button
                      type="button"
                      onClick={() => forget(p)}
                      disabled={busy}
                      className="text-zinc-500 hover:text-zinc-200 cursor-pointer shrink-0"
                      aria-label={`Stop looking in ${p}`}
                      title="Stop looking here"
                    >
                      <X size={13} />
                    </button>
                  </li>
                ))}
                {found?.roots
                  .filter((r) => !extraRoots().some((e) => normPath(e) === normPath(r)) && !(home && normPath(r) === normPath(home)))
                  .map((r) => (
                    <li key={r} className="text-zinc-400 truncate" title={r}>
                      <span className="font-mono text-zinc-300">{short(r)}</span> · holds a profile's folder
                    </li>
                  ))}
                <li className="text-zinc-500">Hidden folders, Library, Pictures, Music, Movies, Videos, package folders and network drives are passed over, and links are not followed.</li>
              </ul>
            )}
            <label className="flex items-center gap-2 text-zinc-300 cursor-pointer select-none">
              <Checkbox checked={autoAdd} onChange={setAutoAdd} disabled={busy} aria-label="Add the ones with one clear account straight away" />
              Add the ones with one clear account straight away
            </label>
          </div>

          {phase === "intro" && (
            <div className="space-y-3">
              <p className="text-zinc-400">
                It reads folder names, and each repository's remote address. Then it asks GitHub, with the accounts you are signed in with, which of them can reach each
                one. Nothing is changed until you add them.
                {isMac && " macOS may ask once whether GitSwitch may look in Desktop, Documents or Downloads."}
              </p>
              <Button onClick={() => void run()}>
                <ScanSearch size={16} />
                Search
              </Button>
            </div>
          )}

          {statusLine && (
            <div className="flex items-center justify-between gap-3">
              <p className="text-xs text-zinc-400 flex items-center gap-2 min-w-0" role="status">
                <Loader2 size={13} className="animate-spin text-emerald-400 shrink-0" />
                <span className="truncate">{statusLine}</span>
              </p>
              {phase !== "adding" && (
                <Button size="sm" variant="ghost" onClick={stop}>
                  <Square size={12} />
                  Stop
                </Button>
              )}
            </div>
          )}

          {error && (
            <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
              {error}
            </p>
          )}

          {added && (
            <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2.5">
              <div className="flex items-center justify-between gap-2 flex-wrap">
                <p className="text-emerald-300 flex items-center gap-2">
                  <CheckCircle2 size={15} className="shrink-0" />
                  Added {plural(added.items.length, "repository", "repositories")}: on the Repository page now.
                </p>
                <Button size="sm" variant="secondary" onClick={() => void undo()} disabled={busy}>
                  <Undo2 size={14} />
                  Undo
                </Button>
              </div>
              <ul className="mt-1.5 text-xs text-emerald-200/80 space-y-0.5">
                {added.items.map((a) => (
                  <li key={a.path} className="truncate font-mono" title={a.path}>
                    {added.labels[a.path] ?? baseName(a.path)}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {found && (found.truncated.length > 0 || found.rejected.length > 0 || found.unreadable.length > 0 || found.linked.length > 0) && (
            <ul className="space-y-1 text-xs text-amber-300/90">
              {found.linked.map((l) => (
                <li key={l.to} className="flex items-center gap-2 flex-wrap">
                  <Link2 size={13} className="shrink-0" />
                  <span className="min-w-0 break-all">
                    <span className="font-mono">{short(l.from)}</span> is a link to <span className="font-mono">{l.to}</span>, which was not searched.
                  </span>
                  <button type="button" className="text-emerald-400 hover:text-emerald-300 cursor-pointer" onClick={() => void lookIn(l.to)} disabled={busy}>
                    Look there too
                  </button>
                </li>
              ))}
              {found.rejected.map((r) => (
                <li key={r.place} className="break-all">
                  Not searched: <span className="font-mono">{r.place}</span> ({r.why}).
                </li>
              ))}
              {found.truncated.map((c) => (
                <li key={c.place} className="break-all">
                  <span className="font-mono">{short(c.place)}</span>: {c.why}. Add the folder your clones are in with Also look in….
                </li>
              ))}
              {found.unreadable.length > 0 && (
                <li className="break-all">
                  Could not read {found.unreadable.map(short).join(", ")}.
                  {isMac && " To search there, allow GitSwitch in System Settings › Privacy & Security › Files and Folders."}
                </li>
              )}
            </ul>
          )}

          {stopped && <p className="text-xs text-zinc-400">Stopped. Search again to look once more.</p>}

          {nothingNew && (
            <p className="text-zinc-300">
              {found!.already_listed > 0
                ? `Every repository in these places is already on the Repository page (${found!.already_listed}).`
                : "No repositories in these places."}
            </p>
          )}

          {rows.length > 0 && (
            <div className="space-y-2">
              <p className="text-xs text-zinc-500 uppercase tracking-wider">
                {plural(rows.length, "repository", "repositories")} no profile's folders hold
                {found && found.already_listed > 0 ? ` · ${found.already_listed} more already listed` : ""}
              </p>
              <div className="space-y-1.5">
                {rows.map((r) => (
                  <div
                    key={r.repo.path}
                    data-find-row={r.repo.path}
                    className="flex items-start gap-3 px-3 py-2.5 rounded-lg bg-zinc-800/50 border border-zinc-700/50 min-w-0 flex-wrap sm:flex-nowrap"
                  >
                    <Folder size={15} className="text-zinc-500 shrink-0 mt-0.5" />
                    <div className="min-w-0 flex-1">
                      <p className="text-zinc-200 font-medium truncate" title={r.repo.path}>
                        {baseName(r.repo.path)}
                        <span className="text-xs text-zinc-500 font-mono font-normal"> · {short(r.repo.path)}</span>
                      </p>
                      <p className="text-xs text-zinc-500 font-mono truncate">
                        {r.repo.owner ? `${r.repo.owner}/${r.repo.name}` : r.repo.remote_url || "no remote"}
                      </p>
                      {badges(r.repo)}
                      <p className="text-xs text-zinc-400 mt-1">{commitsLine(r)}</p>
                      <p className="text-xs text-zinc-500">{r.proposal.reason}</p>
                      {r.proposal.note && <p className="text-xs text-amber-300/90">{r.proposal.note}</p>}
                      {r.proposal.warnOther && r.choice && r.choice !== r.proposal.profileId && (
                        <p className="text-xs text-amber-300/90">This {r.proposal.warnOther}.</p>
                      )}
                    </div>
                    <div className="w-full sm:w-48 shrink-0">
                      <Select
                        value={r.choice}
                        onChange={(v) => setRows((all) => all.map((x) => (x.repo.path === r.repo.path ? { ...x, choice: v } : x)))}
                        placeholder="Don't add"
                        disabled={busy}
                        options={options(r)}
                      />
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

        </div>
        {phase !== "intro" && (
          <div className="flex items-center justify-end gap-2 pt-4 flex-wrap shrink-0">
            <Button variant="secondary" onClick={() => void run()} disabled={busy}>
              <ScanSearch size={16} />
              Search again
            </Button>
            {rows.length > 0 && (
              <Button onClick={() => void add(chosen, profiles).then((done) => setRows((all) => all.filter((r) => !done.has(r.repo.path))))} disabled={busy || chosen.length === 0}>
                Add {plural(chosen.length, "repository", "repositories")} to {chosen.length === 1 ? "its profile" : "their profiles"}
              </Button>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}
