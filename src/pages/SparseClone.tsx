import { useEffect, useReducer, useRef, useState } from "react";
import {
  GitBranch,
  FolderOpen,
  Loader2,
  CheckCircle2,
  Download,
  Folder,
  RefreshCw,
  KeyRound,
  AlertTriangle,
  ShieldCheck,
  ArrowRightLeft,
  X,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useSearchParams } from "react-router-dom";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Checkbox } from "../components/Checkbox";
import { Input } from "../components/Input";
import { Badge } from "../components/Badge";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import { baseName, isWithin, normPath } from "../lib/paths";
import { usePersistedState } from "../lib/persist";
import { useRefreshOnFocus } from "../lib/focus";
import type { Profile } from "../types/profile";
import * as api from "../lib/api";

/** Longest matching folder wins — the same rule the generated gitconfig uses. */
function profileForFolder(dir: string, profiles: Profile[]): Profile | null {
  let best: Profile | null = null;
  let bestLen = -1;
  for (const p of profiles) {
    for (const d of p.directories) {
      const n = normPath(d);
      if (isWithin(dir, n) && n.length > bestLen) {
        best = p;
        bestLen = n.length;
      }
    }
  }
  return best;
}

/** Folder name git will create — mirrors the backend's repo_name_from_url. */
function repoNameFromUrl(url: string): string {
  const t = url.trim().replace(/\/+$/, "").replace(/(\.git)+$/, "");
  const parts = t.split(/[/:]/);
  return parts[parts.length - 1] ?? "";
}

/** https://github.com/o/r(.git) -> git@github.com:o/r.git, else null. */
function httpsToSsh(url: string): string | null {
  const m = url.trim().match(/^https?:\/\/(?:[^@/]+@)?github\.com\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/);
  return m ? `git@github.com:${m[1]}/${m[2]}.git` : null;
}

/**
 * A clone keeps running while the user browses other pages, so its progress is
 * held outside the component. Otherwise leaving and returning loses the
 * "Cloning…" state, and the half-written folder then looks like a finished
 * clone — the page would offer to switch or reuse a repo that is still
 * downloading.
 */
type CloneJob = {
  /** destination path being written */
  key: string;
  name: string;
  mode: "full" | "sparse";
  startedAt: number;
  done?: { result?: api.CloneResult; error?: string };
  /** the result has been shown once */
  consumed?: boolean;
};
let cloneJob: CloneJob | null = null;
const jobListeners = new Set<() => void>();
function publishJob(job: CloneJob | null) {
  cloneJob = job;
  jobListeners.forEach((notify) => notify());
}

export function SparseClone() {
  const toast = useToast();
  const [url, setUrl] = usePersistedState("sparse.url", "");
  const [parentDir, setParentDir] = usePersistedState("sparse.parentDir", "");
  const [folderName, setFolderName] = useState("");
  const [mode, setMode] = usePersistedState<"full" | "sparse">("clone.mode", "full");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [cloned, setCloned] = useState<api.CloneResult | null>(null);
  // "" = automatic: the destination folder's profile decides, like a terminal clone.
  const [cloneAs, setCloneAs] = useState("");
  const [withSubmodules, setWithSubmodules] = usePersistedState("clone.submodules", true);

  // Arriving from Repositories: take the link (and suggested profile) it sent,
  // then drop the query so a reload doesn't re-apply it over later edits.
  const [params, setParams] = useSearchParams();
  const [fromRepo, setFromRepo] = useState<string | null>(null);
  useEffect(() => {
    const u = params.get("url");
    if (!u) return;
    setUrl(u);
    setFolderName("");
    setCloneAs(params.get("cloneAs") ?? "");
    if (params.get("mode") === "full") setMode("full");
    setFromRepo(params.get("from"));
    setCloned(null);
    setParams({}, { replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [params]);

  function reloadProfiles() {
    api.getProfiles().then(setProfiles).catch(() => setProfiles([]));
  }
  useEffect(reloadProfiles, []);
  useEffect(() => {
    if (cloneAs && profiles.length > 0 && !profiles.some((p) => p.id === cloneAs)) setCloneAs("");
  }, [cloneAs, profiles]);
  // A result card describes the clone that just happened — drop it once the
  // form points somewhere else, so it can't be mistaken for the next one.
  // Only an actual change clears it: re-opening the page must not wipe the
  // result of a clone that finished while the user was elsewhere.
  const lastFormKey = useRef(`${url}|${parentDir}`);
  useEffect(() => {
    const key = `${url}|${parentDir}`;
    if (lastFormKey.current === key) return;
    lastFormKey.current = key;
    setCloned(null);
  }, [url, parentDir]);

  // git matches the NEW repo's path (parent/name), not the parent — a more
  // specific profile folder can own that exact path.
  const newFolder = folderName.trim() || repoNameFromUrl(url);
  const destination = parentDir ? (newFolder ? `${parentDir}/${newFolder}` : parentDir) : "";
  const owner = destination ? profileForFolder(destination, profiles) : null;
  const folderProfile = owner ?? profiles.find((p) => p.is_default) ?? null;
  const chosen = cloneAs ? profiles.find((p) => p.id === cloneAs) ?? null : null;
  const effective = chosen ?? folderProfile;
  const willMap = !!chosen && chosen.id !== folderProfile?.id;
  // GitHub shows org-<ID>@github.com links for orgs with an SSH certificate authority.
  const certOrgUrl = /^(?:ssh:\/\/)?org-[^@\s]+@github\.com[:/]/i.test(url.trim());
  const keyInUse = effective?.ssh_key_path ?? null;
  // Bumped when the user returns to the app: the destination folder may have
  // been created or deleted, and a certificate may have appeared, meanwhile.
  const [recheck, setRecheck] = useState(0);
  useRefreshOnFocus(() => setRecheck((n) => n + 1));
  const [cert, setCert] = useState<api.CertInfo | null>(null);
  useEffect(() => {
    if (!certOrgUrl || !keyInUse) {
      setCert(null);
      return;
    }
    let stale = false;
    api.sshCertificateInfo(keyInUse)
      .then((c) => !stale && setCert(c))
      .catch(() => !stale && setCert(null));
    return () => {
      stale = true;
    };
  }, [certOrgUrl, keyInUse, recheck]);

  const [, bumpJob] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    jobListeners.add(bumpJob);
    return () => {
      jobListeners.delete(bumpJob);
    };
  }, []);
  const job = cloneJob;
  const cloning = !!job && !job.done;
  const elapsedSecs = job ? Math.max(0, Math.round((Date.now() - job.startedAt) / 1000)) : 0;
  const elapsedLabel =
    elapsedSecs < 60
      ? `${elapsedSecs}s`
      : `${Math.floor(elapsedSecs / 60)}m ${String(elapsedSecs % 60).padStart(2, "0")}s`;
  // keep the elapsed time ticking while it runs
  useEffect(() => {
    if (!cloning) return;
    const t = setInterval(bumpJob, 1000);
    return () => clearInterval(t);
  }, [cloning]);

  const [dest, setDest] = useState<api.DestinationStatus | null>(null);
  const [destTick, setDestTick] = useState(0);
  const [switching, setSwitching] = useState(false);
  useEffect(() => {
    if (!url.trim() || !parentDir) {
      setDest(null);
      return;
    }
    let stale = false;
    const t = setTimeout(() => {
      api.cloneDestinationStatus(url, parentDir, folderName || null)
        .then((d) => !stale && setDest(d))
        .catch(() => !stale && setDest(null));
    }, 250);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [url, parentDir, folderName, destTick, recheck]);

  // The clone in progress creates its folder immediately; that is not a
  // finished clone, so it must not be offered for switching or reuse.
  const cloningHere = cloning && job?.key === destination;
  const destTaken = !!dest?.exists && cloned?.path !== dest.path && !cloningHere;
  const canSwitch =
    destTaken && !!dest?.same_repo && !dest?.same_url && !dest?.incomplete && !/^https?:\/\//i.test(url.trim());

  async function switchExisting() {
    if (!dest) return;
    setSwitching(true);
    setError(null);
    try {
      toast.success(await api.switchRepoOrigin(dest.path, url));
      setDestTick((n) => n + 1);
    } catch (e) {
      setError(String(e));
    } finally {
      setSwitching(false);
    }
  }

  const blockReason = cloning
    ? `Cloning ${job?.name ?? ""} — wait for it to finish.`
    : destTaken
    ? `${baseName(dest!.path)} already exists in this folder.`
    : chosen && !chosen.ssh_key_path
    ? `Profile "${chosen.name}" has no SSH key — add one to it first.`
    : chosen && /^https?:\/\//i.test(url.trim())
    ? "\"Clone as\" works through SSH, and this is an HTTPS URL — use the SSH URL."
    : null;
  const sshSuggestion = httpsToSsh(url);
  const isHttps = /^https?:\/\//i.test(url.trim());
  const [info, setInfo] = useState<api.SparseInfo | null>(null);
  const [loadingInfo, setLoadingInfo] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function loadInfo(repoPath: string) {
    setLoadingInfo(true);
    setError(null);
    try {
      const i = await api.sparseRepoInfo(repoPath);
      setInfo(i);
      setSelected(new Set(i.sparse_dirs));
    } catch (e) {
      setError(String(e));
      setInfo(null);
    } finally {
      setLoadingInfo(false);
    }
  }

  async function browseParent() {
    const dir = await open({ directory: true, multiple: false, title: "Where should the repo be cloned?" });
    if (typeof dir === "string") setParentDir(dir);
  }

  async function browseExisting() {
    const dir = await open({ directory: true, multiple: false, title: "Select an existing repo folder" });
    if (typeof dir === "string") await loadInfo(dir);
  }

  async function handleClone() {
    if (!url.trim() || !parentDir) {
      setError("Enter a repo URL and choose a destination folder.");
      return;
    }
    if (blockReason) {
      setError(blockReason);
      return;
    }
    if (cloning) {
      setError("A clone is already running — wait for it to finish.");
      return;
    }
    setError(null);
    setCloned(null);
    const name = folderName.trim() || repoNameFromUrl(url);
    const started: CloneJob = {
      key: `${parentDir}/${name}`,
      name,
      mode,
      startedAt: Date.now(),
    };
    publishJob(started);
    const request =
      mode === "full"
        ? api.fullClone(url, parentDir, folderName || null, cloneAs || null, withSubmodules)
        : api.sparseClone(url, parentDir, folderName || null, cloneAs || null);
    request.then(
      (result) => publishJob({ ...started, done: { result } }),
      (e) => publishJob({ ...started, done: { error: String(e) } })
    );
  }

  // Show the outcome once, even if it finished while the user was on another page.
  useEffect(() => {
    if (!job?.done || job.consumed) return;
    job.consumed = true;
    const { result, error: failed } = job.done;
    setDestTick((n) => n + 1);
    if (failed || !result) {
      setError(failed ?? "Clone failed");
      return;
    }
    if (job.mode === "full") {
      setInfo(null);
      setCloned(result);
      toast.success(`Cloned ${baseName(result.path)} as ${result.profile_name ?? "your global identity"}`);
    } else {
      setCloned(result);
      toast.success(
        result.mapped_to
          ? `Cloned (metadata only) as ${result.mapped_to} and added to that profile — now pick folders`
          : "Repository cloned (metadata only) — now pick folders"
      );
      loadInfo(result.path);
    }
    if (result.mapped_to) reloadProfiles();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [job, job?.done]);

  function toggle(dir: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(dir)) next.delete(dir);
      else next.add(dir);
      return next;
    });
  }

  async function handleApply() {
    if (!info) return;
    setApplying(true);
    setError(null);
    try {
      await api.sparseSet(info.path, Array.from(selected));
      toast.success(
        `Checked out ${selected.size} folder${selected.size === 1 ? "" : "s"}`
      );
      await loadInfo(info.path);
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">Clone</h1>
        <p className="text-sm text-zinc-400 mt-1">
          Clone over SSH as the right account — the destination folder's profile
          decides which key is used. Choose a full clone, or a sparse one to
          download only the folders you need.
        </p>
      </div>

      {fromRepo && (
        <div className="flex items-center gap-3 flex-wrap px-4 py-3 rounded-lg bg-emerald-500/5 border border-emerald-500/30 text-sm">
          <span className="text-zinc-300 min-w-0">
            Cloning <span className="font-mono text-emerald-300">{fromRepo}</span> — choose where it
            goes, check the account below, then press Clone.
          </span>
          <button
            onClick={() => setFromRepo(null)}
            className="ml-auto text-zinc-500 hover:text-zinc-300 cursor-pointer"
            aria-label="Dismiss"
          >
            <X size={14} />
          </button>
        </div>
      )}

      {cloning && job && (
        <div className="flex items-start gap-3 px-4 py-3 rounded-lg bg-zinc-800/60 border border-zinc-700/50 text-sm">
          <Loader2 size={16} className="animate-spin text-emerald-400 mt-0.5 shrink-0" />
          <span className="min-w-0">
            <span className="text-zinc-200">
              Cloning <strong>{job.name}</strong>
              {job.mode === "sparse" && " (metadata only)"}…
            </span>
            <span className="block text-xs text-zinc-500 font-mono truncate">{job.key}</span>
            <span className="block text-xs text-zinc-500 mt-0.5">
              Running for {elapsedLabel} — you can use other pages, it keeps going and the result
              appears here.
            </span>
          </span>
        </div>
      )}

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      <Card>
        <div className="flex items-center justify-between gap-3 flex-wrap mb-3">
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider">
            Clone a repo
          </h2>
          <div className="inline-flex rounded-lg bg-zinc-800/80 p-0.5">
            {(["full", "sparse"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`px-3 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer ${
                  mode === m ? "bg-zinc-700 text-emerald-400" : "text-zinc-400 hover:text-zinc-200"
                }`}
              >
                {m === "full" ? "Full clone" : "Sparse (pick folders)"}
              </button>
            ))}
          </div>
        </div>
        <p className="text-xs text-zinc-500 mb-4">
          {mode === "full" ? (
            <>Same as <span className="font-mono">git clone &lt;url&gt;</span> — the whole repo.</>
          ) : (
            <>
              Metadata-only (<span className="font-mono">--filter=blob:none --no-checkout</span>) —
              nothing downloads until you pick folders below.
            </>
          )}{" "}
          Use the SSH URL (<span className="font-mono">git@github.com:…</span>) — on GitHub:{" "}
          <strong className="text-zinc-400">Code → SSH</strong>.
        </p>
        <div className="space-y-3">
          <Input
            label="Repository URL"
            placeholder="git@github.com:owner/repo.git"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          {isHttps && (
            <div className="flex items-start gap-2.5 px-3 py-2.5 rounded-lg bg-amber-500/10 border border-amber-500/30 text-xs text-amber-300">
              <AlertTriangle size={14} className="mt-0.5 shrink-0" />
              <span className="min-w-0 flex-1">
                This is an HTTPS URL, so git won't use your profile's SSH key — it
                authenticates with whatever credential helper is set up, which may
                be a different account.
              </span>
              {sshSuggestion && (
                <Button type="button" size="sm" variant="secondary" onClick={() => setUrl(sshSuggestion)}>
                  Use SSH URL
                </Button>
              )}
            </div>
          )}
          <div className="flex gap-2 items-end">
            <Input
              label="Clone into"
              placeholder="Choose a destination folder…"
              value={parentDir}
              onChange={(e) => setParentDir(e.target.value)}
              containerClassName="flex-1 min-w-0"
            />
            <Button type="button" variant="secondary" onClick={browseParent}>
              <FolderOpen size={16} />
              Browse
            </Button>
          </div>
          <Input
            label="Folder name (optional)"
            placeholder="Defaults to the repo name"
            value={folderName}
            onChange={(e) => setFolderName(e.target.value)}
          />
          {mode === "full" && (
            <label className="flex items-start gap-2.5 cursor-pointer select-none">
              <Checkbox checked={withSubmodules} onChange={setWithSubmodules} className="mt-0.5" />
              <span className="min-w-0">
                <span className="text-sm text-zinc-300">Also download submodules</span>
                <span className="block text-xs text-zinc-500">
                  Same result as <span className="font-mono">git clone --recurse-submodules</span>, with
                  the same account for every submodule. A plain <span className="font-mono">git clone</span>{" "}
                  leaves them empty.
                </span>
              </span>
            </label>
          )}
          <div>
            <label className="block text-sm font-medium text-zinc-300 mb-1.5">Clone as</label>
            <Select
              value={cloneAs}
              onChange={setCloneAs}
              optionIcon={<KeyRound size={14} />}
              options={[
                {
                  value: "",
                  label: folderProfile
                    ? `Automatic — ${folderProfile.name} (decided by the folder)`
                    : "Automatic (decided by the folder)",
                },
                ...profiles.map((p) => ({
                  value: p.id,
                  label: `${p.name} — ${p.git_email}${p.ssh_key_path ? "" : " (no SSH key)"}`,
                })),
              ]}
            />
          </div>
          {parentDir && (
            <div
              className={`flex items-start gap-2.5 px-3 py-2.5 rounded-lg border text-xs min-w-0 ${
                !chosen && !owner
                  ? "bg-amber-500/5 border-amber-500/30"
                  : "bg-zinc-900/60 border-zinc-700/50"
              }`}
            >
              <KeyRound size={14} className="mt-0.5 shrink-0 text-emerald-400" />
              {effective ? (
                <span className="min-w-0">
                  <span className="text-zinc-300">
                    Will clone as <strong>{effective.name}</strong>{" "}
                    <span className="font-mono text-zinc-400">{effective.git_email}</span>
                  </span>
                  <span className="block text-zinc-500 truncate">
                    {effective.ssh_key_path
                      ? <>SSH key <span className="font-mono">{baseName(effective.ssh_key_path)}</span></>
                      : "No SSH key on this profile — git will fall back to your default key"}
                    {destination && <> · into <span className="font-mono">{baseName(destination)}</span></>}
                  </span>
                  {willMap && (
                    <span className="block text-emerald-400/90 mt-0.5">
                      {baseName(destination)} will be added to {chosen!.name}, so pulls and
                      pushes there keep using this account.
                    </span>
                  )}
                  {!chosen && !owner && (
                    <span className="block text-amber-300/90 mt-0.5">
                      No profile covers this folder, so your default profile applies. Cloning a
                      company repo? Pick its profile under “Clone as”.
                    </span>
                  )}
                </span>
              ) : (
                <span className="text-zinc-500">
                  No GitSwitch profile covers this folder — git will use your global
                  identity and default SSH key.
                </span>
              )}
            </div>
          )}
          {destTaken && dest && (
            <div className="flex items-start gap-2.5 px-3 py-2.5 rounded-lg bg-amber-500/5 border border-amber-500/30 text-xs min-w-0">
              <AlertTriangle size={14} className="mt-0.5 shrink-0 text-amber-400" />
              <span className="min-w-0 flex-1">
                {dest.is_repo && dest.incomplete ? (
                  <span className="text-amber-200">
                    <span className="font-mono">{baseName(dest.path)}</span> exists but has no commits —
                    it looks like a clone that was interrupted (or an empty repository). Delete the
                    folder and clone again.
                  </span>
                ) : !dest.is_repo ? (
                  <span className="text-amber-200">
                    <span className="font-mono">{baseName(dest.path)}</span> already exists here and
                    isn't a git repository — choose another folder name.
                  </span>
                ) : !dest.same_repo ? (
                  <span className="text-amber-200">
                    <span className="font-mono">{baseName(dest.path)}</span> already exists here and is
                    a different repository (<span className="font-mono break-all">{dest.origin ?? "no origin"}</span>)
                    — choose another folder name.
                  </span>
                ) : dest.same_url ? (
                  <span className="text-zinc-300">
                    This repo is already cloned here with this exact link — nothing to do.
                  </span>
                ) : (
                  <>
                    <span className="text-amber-200">
                      This repo is already cloned here — no need to download it again.
                    </span>
                    <span className="block text-zinc-400 mt-0.5">
                      Its remote is <span className="font-mono break-all">{dest.origin}</span>
                      {dest.origin_is_https &&
                        " (HTTPS), so pulls and pushes don't go through your SSH key"}
                      .
                    </span>
                  </>
                )}
              </span>
              {canSwitch && (
                <Button type="button" size="sm" variant="secondary" onClick={switchExisting} disabled={switching}>
                  {switching ? <Loader2 size={14} className="animate-spin" /> : <ArrowRightLeft size={14} />}
                  Switch it to this link
                </Button>
              )}
            </div>
          )}
          {certOrgUrl && (
            <div className="flex items-start gap-2.5 px-3 py-2.5 rounded-lg bg-zinc-900/60 border border-zinc-700/50 text-xs min-w-0">
              <ShieldCheck size={14} className="mt-0.5 shrink-0 text-sky-400" />
              <span className="min-w-0">
                <span className="text-zinc-300">This organization uses SSH certificates</span>
                <span className="block text-zinc-500">
                  GitHub shows <span className="font-mono">org-…@github.com</span> links for
                  organizations with a certificate authority. If the organization{" "}
                  <em>requires</em> certificates, GitHub refuses a plain SSH key (and HTTPS).
                </span>
                {keyInUse && cert && (
                  <span
                    className={`block mt-1 ${
                      !cert.exists
                        ? "text-zinc-400"
                        : cert.expired
                        ? "text-amber-300/90"
                        : "text-emerald-400/90"
                    }`}
                  >
                    {!cert.exists ? (
                      <>
                        Just a note, not an error: there's no certificate for{" "}
                        <span className="font-mono">{baseName(keyInUse)}</span>, which is fine unless
                        the organization requires one. Only if GitHub refuses the clone, ask your
                        company admin for a certificate and save it as{" "}
                        <span className="font-mono">{baseName(cert.path)}</span>.
                      </>
                    ) : cert.expired ? (
                      <>
                        The certificate for <span className="font-mono">{baseName(keyInUse)}</span>{" "}
                        expired ({cert.valid_until?.replace("T", " ")}). Get a new one from your company.
                      </>
                    ) : (
                      <>
                        Certificate found for <span className="font-mono">{baseName(keyInUse)}</span>
                        {cert.valid_until === "forever"
                          ? " (no expiry)."
                          : ` — valid until ${cert.valid_until?.replace("T", " ")}.`}
                      </>
                    )}
                  </span>
                )}
              </span>
            </div>
          )}
          <div className="flex items-center gap-3 flex-wrap">
            <Button
              onClick={handleClone}
              disabled={cloning || !!blockReason}
              // measured: "Clone" 94px / "Cloning…" 118px / "Sparse Clone" 144px —
              // reserve the widest of each pair so the label swap shifts nothing
              className={mode === "full" ? "min-w-[8rem]" : "min-w-[9.25rem]"}
            >
              {cloning ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  Cloning…
                </>
              ) : (
                <>
                  <Download size={16} />
                  {mode === "full" ? "Clone" : "Sparse Clone"}
                </>
              )}
            </Button>
            {blockReason && <span className="text-xs text-amber-400">{blockReason}</span>}
            <span className="text-xs text-zinc-500">
              …or manage an already-cloned repo:
            </span>
            <Button type="button" variant="ghost" size="sm" onClick={browseExisting}>
              <FolderOpen size={14} />
              Open existing repo
            </Button>
          </div>
        </div>
      </Card>

      {cloned && mode === "full" && (
        <Card>
          <div className="flex items-center gap-2.5 min-w-0">
            <CheckCircle2 size={18} className="text-emerald-400 shrink-0" />
            <div className="min-w-0">
              <p className="text-sm text-zinc-200">
                Cloned {baseName(cloned.path)}
                {cloned.profile_name && <> as <strong>{cloned.profile_name}</strong></>}
              </p>
              <p className="text-xs text-zinc-500 font-mono truncate">{cloned.path}</p>
              {cloned.mapped_to && (
                <p className="text-xs text-emerald-400/90 mt-0.5">
                  Added to the {cloned.mapped_to} profile — pulls and pushes here use the same key.
                </p>
              )}
              {cloned.submodules && cloned.submodules.listed > 0 && (
                <p
                  className={`text-xs mt-0.5 ${
                    cloned.submodules.downloaded === cloned.submodules.listed
                      ? "text-emerald-400/90"
                      : "text-amber-300/90"
                  }`}
                >
                  Submodules: {cloned.submodules.downloaded} of {cloned.submodules.listed} downloaded.
                </p>
              )}
              {cloned.submodules && cloned.submodules.listed === 0 && (
                <p className="text-xs text-zinc-500 mt-0.5">This repo has no submodules to download.</p>
              )}
              {cloned.submodules && cloned.submodules.unlisted > 0 && (
                <p className="text-xs text-zinc-500 mt-0.5">
                  {cloned.submodules.unlisted} other submodule{" "}
                  {cloned.submodules.unlisted === 1 ? "entry has" : "entries have"} no address in{" "}
                  <span className="font-mono">.gitmodules</span>, so git can't fetch{" "}
                  {cloned.submodules.unlisted === 1 ? "it" : "them"} — those folders stay empty, as
                  with any clone.
                </p>
              )}
              {cloned.submodules?.error && (
                <pre className="text-xs text-amber-300/90 mt-1.5 whitespace-pre-wrap break-words font-sans">
                  {cloned.submodules.error}
                </pre>
              )}
            </div>
          </div>
        </Card>
      )}

      {loadingInfo && (
        <Card>
          <div className="flex items-center justify-center gap-2 py-6 text-sm text-zinc-400">
            <Loader2 size={16} className="animate-spin text-emerald-400" />
            Reading repository folders…
          </div>
        </Card>
      )}

      {info && !loadingInfo && (
        <Card>
          <div className="flex items-center justify-between mb-1 gap-3 flex-wrap">
            <div className="flex items-center gap-2 min-w-0">
              <GitBranch size={16} className="text-emerald-400 shrink-0" />
              <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider truncate">
                {baseName(info.path)}
              </h2>
              <Badge variant="success">{info.branch}</Badge>
              {info.is_sparse && <Badge variant="default">sparse</Badge>}
            </div>
            <button
              onClick={() => loadInfo(info.path)}
              title="Re-read folders"
              className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800 transition-colors cursor-pointer"
            >
              <RefreshCw size={13} />
              Refresh
            </button>
          </div>
          <p className="text-xs text-zinc-500 font-mono mb-4 truncate">{info.path}</p>

          {info.available_dirs.length === 0 ? (
            <p className="text-sm text-zinc-500">
              No top-level folders found in this repo.
            </p>
          ) : (
            <>
              <p className="text-xs text-zinc-500 mb-2">
                Tick the folders to materialize — {selected.size} of{" "}
                {info.available_dirs.length} selected
              </p>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-1.5 mb-4 max-h-72 overflow-y-auto pr-1">
                {info.available_dirs.map((dir) => {
                  const checked = selected.has(dir);
                  const live = info.sparse_dirs.includes(dir);
                  return (
                    <label
                      key={dir}
                      className={`flex items-center gap-2.5 px-3 py-2 rounded-lg border cursor-pointer transition-colors ${
                        checked
                          ? "bg-emerald-500/10 border-emerald-500/40"
                          : "bg-zinc-800/50 border-zinc-700/50 hover:border-zinc-600"
                      }`}
                    >
                      <Checkbox checked={checked} onChange={() => toggle(dir)} />
                      <Folder
                        size={14}
                        className={checked ? "text-emerald-400" : "text-zinc-500"}
                      />
                      <span className="text-sm text-zinc-200 font-mono truncate flex-1">
                        {dir}
                      </span>
                      {live && (
                        <CheckCircle2
                          size={14}
                          className="text-emerald-400 shrink-0"
                          aria-label="currently checked out"
                        />
                      )}
                    </label>
                  );
                })}
              </div>
              <div className="flex items-center justify-between">
                <p className="text-xs text-zinc-600">
                  Equivalent to{" "}
                  <span className="font-mono text-zinc-500">
                    git sparse-checkout set {Array.from(selected).join(" ") || "…"}
                  </span>
                </p>
                <Button onClick={handleApply} disabled={applying || selected.size === 0}>
                  {applying ? (
                    <>
                      <Loader2 size={16} className="animate-spin" />
                      Checking out…
                    </>
                  ) : (
                    "Apply selection"
                  )}
                </Button>
              </div>
            </>
          )}
        </Card>
      )}
    </div>
  );
}
