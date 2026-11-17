import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, AlertTriangle, CircleSlash, Play } from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import { useToast } from "../Toast";
import { TreeLists, type DiscardVariant } from "../changes/TreeLists";
import { CommitBox, type GuardAction } from "../changes/CommitBox";
import { DiffPanel } from "../changes/DiffPanel";
import { TidyUpStrip } from "./TidyUpStrip";
import type { RepoActions } from "./StatusStrip";
import { BranchesModal } from "../changes/BranchesModal";
import type { ApplyOptions, MarkersRetry } from "../changes/SubmoduleSection";
import { PushAccessCard } from "../changes/PushAccessCard";
import { SubmodulesCard } from "../changes/SubmodulesCard";
import { SubmoduleSection, needsSection, sectionId } from "../changes/SubmoduleSection";
import { LfsCard } from "../changes/LfsCard";
import { LfsFilesModal, formatBytes } from "../changes/LfsFilesModal";
import { StashesCard } from "../changes/StashesCard";
import { StatesCard } from "../changes/StatesCard";
import { StashModal } from "../changes/StashModal";
import { SyncPausedCard } from "../changes/SyncCard";
import { ResultCard } from "../changes/ResultCard";
import { DiscardConfirmModal } from "../changes/DiscardConfirmModal";
import { ResetConfirmModal } from "../changes/ResetConfirmModal";
import { DiscardAllModal } from "../changes/DiscardAllModal";
import { AddRemoteModal } from "../changes/AddRemoteModal";
import { cleanError } from "../github/shared";
import { openUrl } from "@tauri-apps/plugin-opener";
import { usePersistedState } from "../../lib/persist";
import { canonicalJoin } from "../../lib/paths";
import { useRefreshOnFocus } from "../../lib/focus";
import { notePushChanged } from "../../lib/console";
import { elapsedLabel, runJob, useGitJob, clearJob, jobWritingFiles, type GitJobKind } from "../../lib/gitJobs";
import * as api from "../../lib/api";
import type { ChangeEntry, OpResult, PullMode, RepoStatus, SubmoduleInfo, SubmoduleStatus } from "../../lib/api";

type DiscardRequest = { paths: string[]; variant: DiscardVariant; target?: string };

interface Props {
  /** The repository — or the submodule in scope — this tab works on. */
  repoPath: string;
  visible: boolean;
  pullMode: PullMode;
  onPullMode: (m: PullMode) => void;
  pullLfs: boolean;
  onPullLfs: (v: boolean) => void;
  pullAutostash: boolean;
  onPullAutostash: (v: boolean) => void;
  /** Open another folder (a submodule as its own repository, or any path) on this page. */
  onOpenRepo: (path: string) => void;
  /** The status this tab read, for the strip above the tabs. */
  onStatus: (s: RepoStatus | null, loading: boolean) => void;
  /** The operations the strip's buttons run. Re-registered on every render. */
  onActions: (a: RepoActions) => void;
  onOpenHistory: () => void;
  /** A run was refused because upstream moved: the drawer re-plans. */
  /** A run on `repo` was refused because upstream moved: re-plan it (and show the drawer if it is the one on screen). */
  onAssessAgain: (repo: string) => void;
  /** A paused update's hand-off: where the conflicts are (needs_user.where_). */
  onGoToConflicts: (where: string) => void;
  /** Opens the Update drawer. */
  onPreview: () => void;
}

/**
 * The working tree: what used to be the whole Changes page, minus its picker
 * and branch line, which now sit above the tabs. It owns the status, the job
 * slot's results and every operation on this repository; the strip above only
 * calls into it.
 */
export function ChangesTab({
  repoPath,
  visible,
  pullMode,
  onPullMode,
  pullLfs,
  onPullLfs,
  pullAutostash,
  onPullAutostash,
  onOpenRepo,
  onStatus,
  onActions,
  onOpenHistory,
  onAssessAgain,
  onGoToConflicts,
  onPreview,
}: Props) {
  const toast = useToast();
  const [drafts, setDrafts] = usePersistedState<Record<string, string>>("changes.drafts", {});

  // Deliberately not persisted: amending must be a fresh decision every time,
  // and status/results must always be re-read rather than restored.
  const [amend, setAmend] = useState(false);
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [subStatuses, setSubStatuses] = useState<SubmoduleStatus[]>([]);
  const [openSubs, setOpenSubs] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<OpResult | null>(null);
  const [diffFor, setDiffFor] = useState<ChangeEntry | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState<DiscardRequest | null>(null);
  const [branchesFor, setBranchesFor] = useState<string | null>(null);
  const [lfsBrowser, setLfsBrowser] = useState(false);
  const [lfsProgress, setLfsProgress] = useState<api.LfsProgress | null>(null);
  const [stashOpen, setStashOpen] = useState(false);
  const [resetOpen, setResetOpen] = useState(false);
  const [discardAllOpen, setDiscardAllOpen] = useState(false);
  /** Publish was refused for want of any remote: the address to add as origin. */
  // "Publish this repository": which tab opens, whether its address tab adds
  // origin or corrects a remote never reached, and the repository the create
  // form starts from. Null: closed.
  const [publishDialog, setPublishDialog] = useState<{
    tab: "create" | "address";
    change?: { remote: string; from: string };
    prefill?: string | null;
  } | null>(null);
  const [tick, setTick] = useState(0);
  const [subRefresh, setSubRefresh] = useState(0);
  // Bumped by Refresh only: the pushing identity may ask github.com, which
  // is not something every Stage or Commit should do.
  const [identityRefresh, setIdentityRefresh] = useState(0);
  /**
   * The retry a "conflict-markers" refusal offers — the very operation that was
   * refused, run again with markers allowed. Set by the call that was refused
   * and cleared by every other result, so it can never pair with a different
   * operation or a different repository.
   */
  const [markersRetry, setMarkersRetry] = useState<MarkersRetry | null>(null);

  const job = useGitJob(repoPath);
  const seq = useRef(0);
  const subSeq = useRef(0);

  /** Each submodule's own status, for its section. Only asked when there are any. */
  const loadSubStatuses = useCallback((path: string, hasSubmodules: boolean) => {
    const mine = ++subSeq.current;
    if (!path || !hasSubmodules) {
      setSubStatuses([]);
      return;
    }
    api
      .changesSubmoduleStatuses(path)
      .then((list) => {
        if (mine === subSeq.current) setSubStatuses(list);
      })
      .catch(() => {
        if (mine === subSeq.current) setSubStatuses([]);
      });
  }, []);

  const loadStatus = useCallback(
    (path: string) => {
      if (!path) {
        setStatus(null);
        setSubStatuses([]);
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
          loadSubStatuses(path, s.has_submodules);
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
    [loadSubStatuses]
  );

  // Switching repos must not show the previous repo's files for a moment.
  useEffect(() => {
    setStatus(null);
    setSubStatuses([]);
    setOpenSubs({});
    setResult(null);
    // A retry belongs to the repository its refusal came from.
    setMarkersRetry(null);
    setAmend(false);
    loadStatus(repoPath);
  }, [repoPath, loadStatus]);

  // After any operation the submodules may have moved too.
  useEffect(() => {
    if (subRefresh === 0) return;
    loadSubStatuses(repoPath, !!status?.has_submodules);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [subRefresh]);

  // Folders change behind the app's back: a terminal commit, a deleted clone.
  // Not while a job is writing this folder: a status read of a repository
  // mid-download competes with the download and made the app lag.
  useRefreshOnFocus(() => {
    if (jobWritingFiles(repoPath)) return;
    loadStatus(repoPath);
  }, 4000);

  // The strip above the tabs shows what this tab knows.
  useEffect(() => {
    onStatus(status, loading);
  }, [status, loading, onStatus]);

  // Keep the elapsed time moving while a job runs.
  useEffect(() => {
    if (!job?.running) return;
    const id = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, [job?.running]);

  // A large-file download can take a long time with nothing on screen moving,
  // which is indistinguishable from the app having hung. git-lfs reports what
  // it is transferring, so ask it rather than leave the person guessing.
  useEffect(() => {
    // Only operations that can download large files. Polling during a push
    // would show the numbers a previous download left behind.
    const downloads = job?.kind === "lfs" || job?.kind === "pull" || job?.kind === "sync";
    if (!job?.running || !downloads || !repoPath) {
      setLfsProgress(null);
      return;
    }
    let stop = false;
    const read = () =>
      api
        .changesLfsProgress(repoPath)
        .then((p) => {
          if (!stop) setLfsProgress(p);
        })
        .catch(() => {
          /* the run may have finished between the poll and the read */
        });
    read();
    const id = setInterval(read, 1000);
    return () => {
      stop = true;
      clearInterval(id);
      setLfsProgress(null);
    };
  }, [job?.running, job?.kind, job?.startedAt, repoPath]);

  // The repository on screen right now, for callbacks that outlive a render:
  // a result that comes back after the person has moved to another
  // repository must not be painted into that one's view.
  const repoRef = useRef(repoPath);
  useEffect(() => {
    repoRef.current = repoPath;
  }, [repoPath]);

  // A finished job is shown by this effect and nowhere else. `job` is the one
  // for the repository on screen, so a job that finished while another
  // repository was showing waits there until the person comes back. Seen
  // jobs are remembered per repository, so returning to one does not replay
  // a result that was already dismissed.
  const seenJob = useRef<Record<string, number>>({});
  /** The current "Continue update", for the job effect below (declared further down). */
  const continueUpdateRef = useRef<(allow: boolean) => unknown>(() => undefined);
  useEffect(() => {
    if (!job || job.running || seenJob.current[repoPath] === job.startedAt) return;
    seenJob.current[repoPath] = job.startedAt;
    if (job.kind === "assess") {
      // The plan lives in its own store; the fetch may have changed the counts.
      loadStatus(repoPath);
      setSubRefresh((n) => n + 1);
      return;
    }
    if (job.result?.refusal?.code === "assess-again") {
      // The drawer re-plans and says why; a result card would only repeat it.
      // Said here as well as where the run was started: the person may have
      // been on another repository when it was refused, and this is the
      // moment it is on screen again. The job is spent once it has been said.
      onAssessAgain(repoPath);
      clearJob(repoPath);
      loadStatus(repoPath);
      return;
    }
    if (job.result) {
      setResult(job.result);
      // A paused update's Continue that was refused for markers while the
      // person was elsewhere: the retry comes back with its refusal.
      if (job.kind === "sync" && job.result.refusal?.code === "conflict-markers") {
        setMarkersRetry({ label: "Continue update anyway", run: () => void continueUpdateRef.current(true) });
      }
      // The result carries the status git had after the operation, so a
      // second full status read on top of it would be wasted work.
      if (job.result.status) setStatus(job.result.status);
      else loadStatus(repoPath);
    }
    if (job.error) {
      setError(job.error);
      loadStatus(repoPath);
    }
    setSubRefresh((n) => n + 1);
  }, [job, repoPath, loadStatus]);

  /** Work in another repo (a submodule) without waiting for the scan. */
  const openRepo = (path: string) => onOpenRepo(path);

  // The identity guard of this repository or of a submodule shown inline:
  // switched here, on any repository, and rewritten when it expects an email
  // the profile no longer has.
  const [guardBusy, setGuardBusy] = useState(false);
  const guardAction = async (path: string, st: RepoStatus, action: GuardAction) => {
    setGuardBusy(true);
    try {
      const msg =
        action === "off"
          ? await api.uninstallCommitGuard(path)
          : await api.installCommitGuard(path, st.identity.profile_email ?? "");
      toast.success(msg);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setGuardBusy(false);
      loadStatus(repoPath);
      setSubRefresh((n) => n + 1);
    }
  };

  const message = drafts[repoPath] ?? "";
  const setMessage = (m: string) => setDrafts((d) => ({ ...d, [repoPath]: m }));
  // What the box shows is what gets committed: with nothing typed, a merge in
  // progress commits with git's own MERGE_MSG rather than an empty message.
  const commitMessage = message || (status?.merge_message ?? "");

  const entries = status?.entries ?? [];
  const dirty = (status?.staged_count ?? 0) + (status?.unstaged_count ?? 0) > 0;

  /**
   * Apply a foreground operation and adopt the status it returns. With a
   * `target` (a submodule's path) the status belongs to that section, and the
   * parent is re-read because its gitlink row just changed too. A refusal the
   * caller named in `opts.quietRefusal` is a question it answers itself (a
   * confirm step), so it gets neither a toast nor a result card.
   */
  const apply = async (key: string, fn: () => Promise<OpResult>, target?: string, opts?: ApplyOptions) => {
    const forRepo = repoPath;
    setBusy(key);
    setError(null);
    try {
      const r = await fn();
      const quiet = !r.ok && !!opts?.quietRefusal && r.refusal?.code === opts.quietRefusal;
      // Still looking at the repository this ran in? Only then is the view
      // updated; the toast below says what happened either way.
      const stillHere = repoRef.current === forRepo;
      if (!quiet && stillHere) {
        // A result from inside a submodule is that submodule's: its paths and
        // codes are relative to it, and no action keyed on them may run here.
        setResult(target && target !== forRepo ? { ...r, inside: target } : r);
        setMarkersRetry(!r.ok && r.refusal?.code === "conflict-markers" && opts?.markers ? opts.markers : null);
      }
      if (r.status && stillHere) {
        const s = r.status;
        if (target && target !== forRepo) {
          setSubStatuses((list) => list.map((x) => (x.status.path === target ? { ...x, status: s } : x)));
          loadStatus(forRepo);
        } else {
          setStatus(s);
        }
      }
      if (stillHere) setSubRefresh((n) => n + 1);
      if (r.ok) {
        toast.success(r.headline);
      } else if (quiet) {
        // the caller asks the question in its own dialog
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

  /**
   * Long operations survive leaving the page, so they run in the job slot.
   * Painting the result is left to the job effect above, which is keyed to
   * the repository on screen — so a pull that finishes after the person has
   * moved to another repository is shown when they come back, not over the
   * one they are looking at.
   */
  const runLongIn = (repo: string, kind: GitJobKind, label: string, fn: () => Promise<OpResult>, markers?: MarkersRetry) => {
    setResult(null);
    setError(null);
    setMarkersRetry(null);
    return runJob(repo, kind, label, fn).then((r) => {
      if (!r) return;
      if (repoRef.current === repo) setMarkersRetry(!r.ok && r.refusal?.code === "conflict-markers" && markers ? markers : null);
      if (r.ok) toast.success(r.headline);
      else if (r.refusal?.code === "assess-again") onAssessAgain(repo);
      // A pause is a hand-off, not a failure.
      else if (r.sync && r.sync.phase === "needs-user") toast.info(r.headline);
      // Finished with a check that did not pass: its own headline says which.
      else toast.error(r.refusal?.message ?? r.advice?.headline ?? (r.sync ? r.headline : "That didn't work"));
    });
  };
  /** Opens a file in the system's default application, or reveals it. */
  const openPath = (path: string, reveal: boolean) =>
    void api.changesOpenPath(repoPath, path, reveal).catch((e) => toast.error(String(e)));
  const runLong = (kind: GitJobKind, label: string, fn: () => Promise<OpResult>, markers?: MarkersRetry) => runLongIn(repoPath, kind, label, fn, markers);
  // The three operations the marker guard can refuse, each carrying its own
  // "…anyway" — the same call with markers allowed, on the same repository.
  const stagePaths = (paths: string[], allow: boolean) =>
    apply("stage", () => api.changesStage(repoPath, paths, allow), undefined, allow ? undefined : { markers: { label: "Mark resolved anyway", run: () => void stagePaths(paths, true) } });
  const continueOp = (allow: boolean) =>
    apply("continue", () => api.changesContinue(repoPath, allow), undefined, allow ? undefined : { markers: { label: "Continue anyway", run: () => void continueOp(true) } });
  const continueUpdate = (allow: boolean) =>
    runLong("sync", "Continuing the update…", () => api.changesSyncContinue(repoPath, allow), allow ? undefined : { label: "Continue update anyway", run: () => void continueUpdate(true) });
  continueUpdateRef.current = continueUpdate;

  /**
   * `git pull` inside one submodule, on its own branch, with the page's pull
   * settings. It is a foreground operation on that submodule (`apply` with a
   * target): the result carries the submodule's status, never the parent's,
   * and the parent is re-read because its gitlink row moves.
   */
  const pullInside = (repo: string, s: SubmoduleInfo, source?: string) => {
    const abs = `${repo.replace(/\/+$/, "")}/${s.path}`;
    // With a source: a rebase onto it ("Rebase onto origin/main"), whatever the page's pull mode.
    const mode = source ? "rebase" : pullMode;
    return apply(
      `pull:${s.path}`,
      // The page's option, as it is: whether LFS is used is the submodule's own
      // matter, which the backend reads there (nothing to do where it is not).
      () => api.changesPull(abs, mode, pullLfs, mode === "rebase" && pullAutostash, source ?? null),
      abs
    );
  };

  const onCommit = async () => {
    const r = await apply("commit", () => api.changesCommit(repoPath, commitMessage, amend));
    if (r?.ok) {
      setDrafts((d) => ({ ...d, [repoPath]: "" }));
      setAmend(false);
    }
  };

  const discardNow = async () => {
    const c = confirmDiscard;
    setConfirmDiscard(null);
    if (!c) return;
    await apply("discard", () => api.changesDiscard(c.target ?? repoPath, c.paths), c.target);
  };

  const running = job?.running ?? false;
  const anyBusy = busy !== null || running;

  // The strip's buttons: fresh closures every render, stored in a ref upstairs.
  useEffect(() => {
    onActions({
      fetch: () =>
        runLong("fetch", "Fetching…", async () => {
          await api.historyFetch(repoPath);
          const s = await api.changesRepoStatus(repoPath);
          return {
            ok: true,
            headline: "Fetched.",
            detail: `${s.behind} commit(s) waiting to be pulled.`,
            status: s,
          };
        }),
      pull: () =>
        status
          ? runLong(
              "pull",
              status.uses_lfs && pullLfs ? `Pulling (${pullMode}) and downloading LFS files…` : `Pulling (${pullMode})…`,
              () => api.changesPull(repoPath, pullMode, status.uses_lfs && pullLfs, pullMode === "rebase" && pullAutostash)
            )
          : undefined,
      push: () => {
        if (!status) return undefined;
        // A repository with no remote yet: where to publish comes first, here in the app.
        if (status.push.remotes.length === 0 && status.branch && !status.unborn && !status.push.blocked) {
          setPublishDialog({ tab: "create" });
          return undefined;
        }
        return runLong("push", "Pushing…", () => api.changesPush(repoPath, !status.upstream));
      },
      branches: () => setBranchesFor(repoPath),
      // Everything a fresh visit reads: the status, the submodules, the
      // stashes and the pushing identity.
      refresh: () => {
        loadStatus(repoPath);
        setSubRefresh((k) => k + 1);
        setIdentityRefresh((k) => k + 1);
      },
      initialise: (repo, paths) =>
        runLongIn(repo, "submodules", `Downloading ${paths.length} submodule${paths.length === 1 ? "" : "s"}…`, () =>
          api.changesSubmoduleUpdate(repo, paths)
        ),
      updateOne: (repo, sub) => runLongIn(repo, "submodules", `Updating ${sub}…`, () => api.changesSubmoduleUpdate(repo, [sub])),
      pullInside,
      recordPointers: (repo) => apply("record", () => api.changesRecordPointers(repo)),
      recordOne: (repo, sub) => apply(`record:${sub}`, () => api.changesRecordPointers(repo, [sub])),
      alignOne: (repo, sub) => runLongIn(repo, "submodules", `Checking out ${sub}'s recorded commit…`, () => api.changesSubmoduleAlign(repo, [sub])),
      startBranch: (repo, sub, name) => startBranch(repo, sub, name),
      // The page's large-file choice, as Pull takes it: the backend decides
      // which repositories of the update use LFS.
      update: (repo, stash, bundles, fingerprint) =>
        runLongIn(
          repo,
          "sync",
          pullLfs ? "Updating — fetching, rebasing, aligning submodules, then the LFS files…" : "Updating — fetching, rebasing, aligning submodules…",
          () => api.changesSyncRun(repo, stash, bundles, fingerprint, pullLfs)
        ),
      stashNow: (repo) => apply("stash", () => api.changesStashPush(repo, "Before update", false)),
    });
  });

  // What the result card offers next: a paused update hands over to its
  // conflicts; a pull that moved submodule pointers offers the update.
  const syncHandOff = result?.sync?.needs_user
    ? {
        label: result.sync.needs_user.where_.startsWith("submodule")
          ? `Open ${result.sync.needs_user.where_.split(":")[1] || "the submodule"}`
          : "Go to the conflicts",
        onClick: () => onGoToConflicts(result.sync?.needs_user?.where_ ?? "superproject"),
        primary: true,
      }
    : null;
  const movedAction = result?.pull && !result.inside && (result.pull.submodules_moved ?? 0) > 0 ? { label: "Update…", onClick: onPreview, primary: true } : null;
  // A file staged with its conflict markers still in it: the refusal names
  // the lines, and whoever meant it (a file that really contains "<<<<<<<")
  // says so once.
  const markersAction = result?.refusal?.code === "conflict-markers" && markersRetry ? { label: markersRetry.label, onClick: markersRetry.run } : null;

  // The entries the discard confirmation describes: the parent's, or the submodule's.
  const confirmEntries = confirmDiscard?.target
    ? subStatuses.find((s) => s.status.path === confirmDiscard.target)?.status.entries ?? []
    : entries;

  // Submodules with something to do get a section; the rest are one line.
  const sectionSubs = subStatuses.filter((s) => needsSection(s.status));
  const quietSubs = subStatuses.length - sectionSubs.length;
  const sectionPaths = sectionSubs.map((s) => s.path);
  const isSubOpen = (s: SubmoduleStatus) =>
    openSubs[s.path] ?? (s.status.conflicted_count > 0 || sectionSubs.length <= 2);
  const jumpToSubmodule = (path: string) => {
    setOpenSubs((o) => ({ ...o, [path]: true }));
    // After the body has rendered, so the section lands at the top of the view.
    setTimeout(() => {
      document.getElementById(sectionId(path))?.scrollIntoView({ behavior: "smooth", block: "start" });
    }, 50);
  };

  // The branch picker can serve the parent or one submodule.
  const branchesStatus =
    branchesFor === null
      ? null
      : branchesFor === repoPath
        ? status
        : subStatuses.find((s) => s.status.path === branchesFor)?.status ?? null;

  // A branch at a detached submodule's checkout: the result is the
  // submodule's, so it updates that row's status, not the repository's.
  const startBranch = (repo: string, sub: string, name: string) => {
    const inside = canonicalJoin(repo, sub);
    return apply(`branch:${sub}`, () => api.changesCreateBranch(inside, name, null, true), inside);
  };

  // A result that belongs to a submodule (a push chained from here, or any
  // operation run inside one) carries its codes for the submodule, not for
  // this repository: no action keyed on them is offered for it.
  const refusalCode = result?.inside ? undefined : result?.refusal?.code;
  // The submodule whose branch form the card should open (a nonce, then the path).
  const [branchFormFor, setBranchFormFor] = useState<string | null>(null);
  // A push refused because a submodule's recorded commit is on no remote of
  // its own: one button per submodule pushes it first and then this
  // repository — or, for a detached submodule, opens its branch form, since a
  // push from it needs a branch; pushing anyway is the person's call.
  const subPushActions =
    refusalCode === "submodules-unpushed" && status
      ? [
          ...(result?.unpushed_submodules ?? [])
            .filter((s) => s.state === "not-on-remote")
            .map((s) =>
              s.branch === null
                ? {
                    label: `Start a branch in ${s.path} first`,
                    primary: true,
                    onClick: () => setBranchFormFor(`${Date.now()}:${s.path}`),
                  }
                : {
                    label: `Push ${s.path} first`,
                    primary: true,
                    onClick: () =>
                      runLong("push", `Pushing ${s.path}, then this repository…`, async () => {
                        const inside = canonicalJoin(repoPath, s.path);
                        // Its upstream as of now, not as of the page's last read.
                        const st = await api.changesRepoStatus(inside).catch(() => null);
                        const r = await api.changesPush(inside, !st?.upstream);
                        // The submodule's own refusal, named for it everywhere
                        // it is read (card and toast); its status is not this
                        // repository's.
                        if (!r.ok) {
                          return {
                            ...r,
                            inside,
                            headline: `${s.path}: ${r.headline}`,
                            refusal: r.refusal ? { ...r.refusal, message: `${s.path}: ${r.refusal.message}` } : undefined,
                            advice: r.advice ? { ...r.advice, headline: `${s.path}: ${r.advice.headline}` } : undefined,
                            status: undefined,
                          };
                        }
                        return api.changesPush(repoPath, !status.upstream);
                      }),
                  }
            ),
          { label: "Push anyway", onClick: () => runLong("push", "Pushing…", () => api.changesPush(repoPath, !status.upstream, true)) },
        ]
      : undefined;
  const historyAction =
    refusalCode === "already-pushed" || refusalCode === "would-drop-pushed"
      ? {
          label: "Open in History",
          onClick: () => {
            onOpenHistory();
          },
        }
      : undefined;
  // A branch whose upstream has another name (or is a local branch) is not
  // pushed there; the way out is to publish it under its own name.
  const publishAction =
    (refusalCode === "upstream-name-differs" || refusalCode === "local-upstream") && status?.branch
      ? {
          label: `Publish as ${result?.refusal?.message.match(/`([^`]+\/[^`]+)` instead/)?.[1] ?? status.branch}`,
          primary: true,
          onClick: () => runLong("push", "Publishing…", () => api.changesPush(repoPath, true)),
        }
      : undefined;
  // No remote at all (a repository made here, say): create it on GitHub, or an address.
  const noRemoteActions =
    refusalCode === "no-remote" && status?.branch
      ? [
          { label: "Create it on GitHub…", primary: true, onClick: () => setPublishDialog({ tab: "create" }) },
          { label: "Use an address…", onClick: () => setPublishDialog({ tab: "address" }) },
        ]
      : undefined;
  // A push or fetch that found no repository at the address: GitHub's form to
  // create it (under the account's own name), and, while nothing was ever
  // fetched from or pushed to that remote, a corrected address.
  const createRepoUrl = result && !result.inside ? (result.advice?.link ?? null) : null;
  const changeFrom =
    result && !result.inside && result.advice?.action === "change-remote" && result.advice.remote
      ? status?.push.remotes.find((r) => r.name === result.advice?.remote)
      : undefined;
  // Created here when the remote was never reached (the form then corrects
  // its address if the name differs); in the browser otherwise.
  const missingRepoActions = [
    ...(createRepoUrl
      ? [
          {
            label: "Create it on GitHub",
            primary: true,
            onClick: () =>
              changeFrom
                ? setPublishDialog({ tab: "create", change: { remote: changeFrom.name, from: changeFrom.fetch_url }, prefill: result?.advice?.repo ?? null })
                : void openUrl(createRepoUrl).catch(() => toast.error(`Couldn't open the browser: open ${createRepoUrl} yourself.`)),
          },
        ]
      : []),
    ...(changeFrom
      ? [{ label: "Change the address…", primary: !createRepoUrl, onClick: () => setPublishDialog({ tab: "address", change: { remote: changeFrom.name, from: changeFrom.fetch_url } }) }]
      : []),
  ];

  return (
    <>
    <div className={`space-y-5 ${visible ? "" : "hidden"}`} role="tabpanel" aria-label="Changes">

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
            {lfsProgress && (
              <span className="min-w-0 truncate text-xs text-sky-300/90" title={lfsProgress.file}>
                {/* git-lfs counts bytes per file, not across the transfer, so
                    they are shown as that one file's progress. */}
                · {lfsProgress.done} of {lfsProgress.total} files · {lfsProgress.file} (
                {formatBytes(lfsProgress.bytes)} of {formatBytes(lfsProgress.total_bytes)})
              </span>
            )}
            <span className="text-xs text-zinc-500 ml-auto">
              keeps running if you leave this page
            </span>
          </div>
        </Card>
      )}

      {!repoPath && (
        <Card>
          <p className="text-sm text-zinc-400">Pick a repository above to see what has changed in it.</p>
        </Card>
      )}

      {status && (
        <>
          {status.sync && (
            <SyncPausedCard
              sync={status.sync}
              status={status}
              busy={anyBusy}
              onContinue={() => void continueUpdate(false)}
              onAbort={() => runLong("sync", "Aborting the update…", () => api.changesSyncAbort(repoPath))}
              onOpenRoot={() => status.sync && openRepo(status.sync.root)}
              onHandOff={onGoToConflicts}
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
                    onClick={() => void continueOp(false)}
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

          {/* How a pull behaves, and the tidy-up strip. The branch line and the
              Fetch / Pull / Push buttons live on the strip above the tabs. */}
          <TidyUpStrip
            status={status}
            busy={anyBusy}
            dirty={dirty}
            pullMode={pullMode}
            onPullMode={onPullMode}
            pullLfs={pullLfs}
            onPullLfs={onPullLfs}
            lfsInSubmodules={subStatuses.some((s) => s.status.uses_lfs)}
            pullAutostash={pullAutostash}
            onPullAutostash={onPullAutostash}
            onUndoCommit={() => apply("undo", () => api.changesUndoCommit(repoPath))}
            onStash={() => setStashOpen(true)}
            onReset={() => setResetOpen(true)}
            onDiscardAll={() => setDiscardAllOpen(true)}
          />

          {result && (
            <ResultCard
              result={result}
              busy={anyBusy}
              onDismiss={() => {
                setResult(null);
                clearJob(repoPath);
              }}
              onRecordPointers={() => apply("record", () => api.changesRecordPointers(repoPath))}
              // A result from inside a submodule names paths inside it: aligned there.
              onAlignSubmodules={(paths) => {
                const inside = result.inside;
                if (inside) void apply("align", () => api.changesSubmoduleAlign(inside, paths), inside);
                else void runLong("submodules", `Checking out the recorded commit${paths.length === 1 ? "" : "s"}…`, () => api.changesSubmoduleAlign(repoPath, paths));
              }}
              extraAction={markersAction ?? syncHandOff ?? movedAction ?? historyAction ?? publishAction}
              extraActions={subPushActions ?? noRemoteActions ?? (missingRepoActions.length > 0 ? missingRepoActions : undefined)}
            />
          )}

          {/* items-start only in row mode: in the stacked (column) layout it is the
              cross axis, so it would size each column to its content and let a long
              file path widen the whole page. */}
          <div className="flex flex-col lg:flex-row gap-4 lg:items-start">
            <div className="flex-1 min-w-0 space-y-3">
              <TreeLists
                status={status}
                busy={anyBusy}
                onOpen={setDiffFor}
                onStage={(paths) => void stagePaths(paths, false)}
                onUnstage={(paths) => apply("unstage", () => api.changesUnstage(repoPath, paths))}
                onDiscard={(paths, variant) => setConfirmDiscard({ paths, variant })}
                onResolveSide={(paths, side) =>
                  apply("resolve", () => api.changesResolveSide(repoPath, paths, side))
                }
                onOpenPath={openPath}
                jumpableSubmodules={sectionPaths}
                onJumpToSubmodule={jumpToSubmodule}
              />

              <CommitBox
                status={status}
                message={commitMessage}
                onMessage={setMessage}
                amend={amend}
                onAmend={setAmend}
                busy={anyBusy}
                onCommit={onCommit}
                onSwitchBranch={() => setBranchesFor(repoPath)}
                onGuard={(a) => guardAction(repoPath, status, a)}
                guardBusy={guardBusy}
              />

              {sectionSubs.map((s) => (
                <SubmoduleSection
                  key={s.path}
                  sub={s}
                  parentRepoPath={repoPath}
                  parentStatus={status}
                  open={isSubOpen(s)}
                  onToggle={() => setOpenSubs((o) => ({ ...o, [s.path]: !isSubOpen(s) }))}
                  busy={anyBusy}
                  draft={drafts[s.status.path] ?? ""}
                  onDraft={(m) => setDrafts((d) => ({ ...d, [s.status.path]: m }))}
                  apply={apply}
                  onOpenRepo={openRepo}
                  onDiscard={(paths, variant) => setConfirmDiscard({ paths, variant, target: s.status.path })}
                  onSwitchBranch={() => setBranchesFor(s.status.path)}
                  onGuard={(a) => guardAction(s.status.path, s.status, a)}
                  guardBusy={guardBusy}
                />
              ))}
              {quietSubs > 0 && (
                <p className="text-xs text-zinc-500">
                  {quietSubs} {sectionSubs.length > 0 ? "other " : ""}submodule{quietSubs === 1 ? "" : "s"}{" "}
                  {quietSubs === 1 ? "has" : "have"} nothing to commit.
                </p>
              )}
            </div>

            <div className="w-full lg:w-80 shrink-0 space-y-3">
              <PushAccessCard
                repoPath={repoPath}
                refreshKey={identityRefresh}
                push={status.push}
                onChanged={(p) => {
                  // The terminal drawer's badge reads it again, whichever
                  // repository this page shows by now.
                  notePushChanged();
                  // A mode change that lands after a switch to another
                  // repository must not carry this status over to it.
                  if (repoRef.current !== repoPath) return;
                  setStatus((s) => (s ? { ...s, push: p } : s));
                }}
              />

              {status.uses_lfs && (
                <LfsCard
                  repoPath={repoPath}
                  busy={anyBusy}
                  refreshKey={subRefresh}
                  onPull={() =>
                    runLong("lfs", "Downloading LFS files…", () => api.changesLfsPull(repoPath))
                  }
                  onBrowse={() => setLfsBrowser(true)}
                />
              )}

              <SubmodulesCard
                repoPath={repoPath}
                busy={anyBusy}
                refreshKey={subRefresh}
                onOpenRepo={openRepo}
                onUpdate={(paths) =>
                  runLong("submodules", `Downloading ${paths.length} submodule${paths.length === 1 ? "" : "s"}…`, () =>
                    api.changesSubmoduleUpdate(repoPath, paths)
                  )
                }
                onPullInside={(s, source) => void pullInside(repoPath, s, source)}
                onUpdateOne={(p) => void runLong("submodules", `Updating ${p}…`, () => api.changesSubmoduleUpdate(repoPath, [p]))}
                onRecordOne={(p) => void apply(`record:${p}`, () => api.changesRecordPointers(repoPath, [p]))}
                onAlignOne={(p) => void runLong("submodules", `Checking out ${p}'s recorded commit…`, () => api.changesSubmoduleAlign(repoPath, [p]))}
                onStartBranch={(p, name) => void startBranch(repoPath, p, name)}
                openBranchFormFor={branchFormFor}
                sectionPaths={sectionPaths}
                onJumpToSection={jumpToSubmodule}
              />

              <StatesCard
                repoPath={repoPath}
                status={status}
                busy={anyBusy}
                refreshKey={subRefresh}
                run={(key, fn) => apply(key, fn)}
              />

              <StashesCard
                repoPath={repoPath}
                status={status}
                busy={anyBusy}
                refreshKey={subRefresh}
                run={(key, fn) => apply(key, fn)}
              />
            </div>
          </div>
        </>
      )}

      {diffFor && status && (
        <DiffPanel repoPath={repoPath} entry={diffFor} onClose={() => setDiffFor(null)} onOpenPath={openPath} />
      )}

      <DiscardConfirmModal
        open={confirmDiscard !== null}
        paths={confirmDiscard?.paths ?? []}
        entries={confirmEntries}
        variant={confirmDiscard?.variant ?? "changes"}
        onCancel={() => setConfirmDiscard(null)}
        onConfirm={discardNow}
      />

      {status && (
        <>
          <StashModal
            open={stashOpen}
            status={status}
            onCancel={() => setStashOpen(false)}
            onConfirm={(msg, includeUntracked) => {
              setStashOpen(false);
              void apply("stash", () => api.changesStashPush(repoPath, msg, includeUntracked));
            }}
          />
          <ResetConfirmModal
            open={resetOpen}
            status={status}
            onCancel={() => setResetOpen(false)}
            onConfirm={(stashFirst) => {
              setResetOpen(false);
              void apply("reset", () => api.changesReset(repoPath, "@{u}", "hard", stashFirst));
            }}
          />
          <LfsFilesModal
            open={lfsBrowser}
            repoPath={repoPath}
            busy={anyBusy}
            onClose={() => setLfsBrowser(false)}
            onDownload={(paths) =>
              runLong(
                "lfs",
                `Downloading ${paths.length === 1 ? paths[0] : `${paths.length} selected paths`}…`,
                () => api.changesLfsPullPaths(repoPath, paths)
              )
            }
            onDownloadAll={() =>
              runLong("lfs", "Downloading LFS files…", () => api.changesLfsPull(repoPath))
            }
          />
          <AddRemoteModal
            open={publishDialog !== null}
            mode={publishDialog?.change ? "change" : "add"}
            initialUrl={publishDialog?.change?.from ?? ""}
            initialTab={publishDialog?.tab ?? "create"}
            repoPath={repoPath}
            prefill={publishDialog?.prefill ?? null}
            onCancel={() => setPublishDialog(null)}
            onAlreadyConnected={() => {
              setPublishDialog(null);
              void runLong("push", "Pushing…", () => api.changesPush(repoPath, !status?.upstream));
            }}
            onConfirm={async (url, allowExisting) => {
              const change = publishDialog?.change;
              let check: api.RemoteCheck;
              try {
                check = change
                  ? await api.changesChangeRemote(repoPath, change.remote, change.from, url, allowExisting)
                  : await api.changesAddRemote(repoPath, url, allowExisting);
              } catch (e) {
                return { text: cleanError(e) };
              }
              if (!check.saved) return { text: check.problem, createUrl: check.create_url, existingBranches: check.existing_branches, repo: check.repo };
              setPublishDialog(null);
              void runLong("push", "Publishing…", () => api.changesPush(repoPath, true));
              return null;
            }}
            onCreated={(p) => {
              setPublishDialog(null);
              toast.success(`Created ${p.full_name} on GitHub (${p.private ? "private" : "public"})${p.renamed ? ": GitHub changed the name" : ""}.`);
              if (p.problem) {
                setError(p.problem);
                loadStatus(repoPath);
                return;
              }
              void runLong("push", "Publishing…", () => api.changesPush(repoPath, true));
            }}
          />
          <DiscardAllModal
            open={discardAllOpen}
            status={status}
            onCancel={() => setDiscardAllOpen(false)}
            onConfirm={(includeUntracked, stashFirst) => {
              setDiscardAllOpen(false);
              void apply("discard", () => api.changesDiscardAll(repoPath, includeUntracked, stashFirst));
            }}
          />
        </>
      )}

      {/* tick keeps the elapsed timer honest without re-rendering anything else */}
      <span className="hidden">{tick}</span>
    </div>

    {/* Outside the panel: the strip's Branches button is on every tab, and a
        dialog inside a hidden panel opened unseen — the click seemed to do nothing. */}
    {branchesFor !== null && branchesStatus && (
      <BranchesModal
        open
        onClose={() => setBranchesFor(null)}
        repoPath={branchesFor}
        status={branchesStatus}
        busy={anyBusy}
        run={(key, fn, opts) => apply(key, fn, branchesFor === repoPath ? undefined : branchesFor, opts)}
      />
    )}
    </>
  );
}
