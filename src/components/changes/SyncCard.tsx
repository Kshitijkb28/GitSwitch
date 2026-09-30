import { useEffect, useState } from "react";
import {
  AlertTriangle,
  ArrowDownUp,
  CheckCircle2,
  CircleSlash,
  GitBranch,
  Loader2,
  Play,
  RefreshCw,
  XCircle,
} from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import { Checkbox } from "../Checkbox";
import { baseName } from "../../lib/paths";
import { usePersistedState } from "../../lib/persist";
import * as api from "../../lib/api";
import type { RepoStatus, SyncOutcome, SyncPlan, SyncSubPlan, SyncSummary } from "../../lib/api";

const short = (oid: string | null | undefined) => (oid ? oid.slice(0, 7) : "—");

/** The plan's action, in the words a person would use. */
function actionLabel(s: SyncSubPlan): string {
  switch (s.action) {
    case "none":
      return "nothing to do";
    case "fast-forward":
      return `fast-forward to ${short(s.target)}`;
    case "rebase":
      return `rebase your commits onto ${short(s.rebase_onto)}`;
    case "ahead":
      return "left alone (ahead)";
    case "blocked":
      return "blocked";
    default:
      return s.action;
  }
}

function whyNoAssess(status: RepoStatus): string | null {
  if (status.unborn) return "No commits yet.";
  // Before the detached check: a rebase detaches HEAD, and the remedy for that
  // is to finish or abort it, not to switch branches.
  if (status.operation) return `Finish or abort the ${status.operation.kind} first.`;
  if (status.detached) return "Check out a branch first — a detached HEAD has nothing to replay onto.";
  if (!status.upstream) return "This branch has no upstream to sync from. Publish it first.";
  if (status.conflicted_count > 0) return "Resolve the conflicts first.";
  return null;
}

type Props = {
  repoPath: string;
  status: RepoStatus;
  busy: boolean;
  /** Runs the plan as a long job; the result arrives through the page's result card. */
  onSync: (stash: boolean, bundles: boolean, fingerprint: [string, string][]) => void;
  /** Offered next to "Check out a branch first": opens the branch picker. */
  onSwitchBranch?: () => void;
};

/**
 * "Take the latest, keep my commits on top" — for any repository, submodules
 * included. Two steps on purpose: Assess fetches and shows exactly what would
 * happen; Sync now does it. Nothing here ever pushes.
 */
export function SyncCard({ repoPath, status, busy, onSync, onSwitchBranch }: Props) {
  const [stash, setStash] = usePersistedState("changes.syncStash", true);
  const [bundles, setBundles] = usePersistedState("changes.syncBundles", false);
  const [plan, setPlan] = useState<SyncPlan | null>(null);
  const [assessing, setAssessing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A plan describes one repository at one HEAD. Anything else is stale.
  useEffect(() => {
    setPlan(null);
    setError(null);
  }, [repoPath, status.head_oid, status.upstream]);

  const assess = async (fetch: boolean, stashChoice: boolean) => {
    setAssessing(true);
    setError(null);
    try {
      setPlan(await api.changesSyncPlan(repoPath, stashChoice, fetch));
    } catch (e) {
      setError(String(e));
    } finally {
      setAssessing(false);
    }
  };

  const onStash = (v: boolean) => {
    setStash(v);
    // The blockers depend on this choice; re-plan locally, no second fetch.
    if (plan && !plan.refusal) void assess(false, v);
  };

  const blocked = whyNoAssess(status);
  const sup = plan?.superproject;

  return (
    <Card>
      <div className="space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <h3 className="text-sm font-medium text-zinc-100 flex items-center gap-2">
              <ArrowDownUp size={15} className="text-zinc-400 shrink-0" />
              Sync: the latest from {status.upstream ?? "upstream"}, your commits on top
            </h3>
            <p className="text-xs text-zinc-400 mt-1 leading-relaxed">
              Fetches, then replays your commits on top of what came in — rebasing the
              submodules that carry your work and fast-forwarding the rest. A backup branch
              is made first. Nothing is pushed.
            </p>
          </div>
          <Button
            size="sm"
            variant={plan ? "secondary" : "primary"}
            disabled={busy || assessing || blocked !== null}
            title={blocked ?? "Fetches from the remote, then shows the plan. Changes nothing."}
            onClick={() => assess(true, stash)}
            className="min-w-[9rem]"
          >
            {assessing ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            {assessing ? "Assessing…" : plan ? "Assess again" : "Assess (fetches)"}
          </Button>
        </div>

        {blocked && !plan && (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-xs text-amber-400/90">{blocked}</p>
            {status.detached && !status.operation && onSwitchBranch && (
              <Button size="sm" variant="ghost" disabled={busy} onClick={onSwitchBranch}>
                <GitBranch size={13} />
                Choose a branch
              </Button>
            )}
          </div>
        )}
        {error && <p className="text-xs text-red-300 break-words">{error}</p>}

        {plan && sup && (
          <div className="space-y-3 border-t border-zinc-800 pt-3">
            <p className={`text-sm ${plan.refusal ? "text-red-200" : plan.nothing_to_do ? "text-zinc-300" : "text-zinc-100"}`}>
              {plan.summary}
            </p>

            {plan.blockers.length > 0 && (
              <ul className="space-y-1" aria-label="Blockers">
                {plan.blockers.map((b) => (
                  <li key={b} className="flex items-start gap-2 text-xs text-red-300">
                    <XCircle size={13} className="shrink-0 mt-0.5" />
                    <span className="break-words">{b}</span>
                  </li>
                ))}
              </ul>
            )}

            {!plan.refusal && (
              <div className="text-xs text-zinc-400 space-y-1">
                <p>
                  <span className="text-zinc-200">{sup.branch}</span> is {sup.ahead} ahead and{" "}
                  {sup.behind} behind {sup.upstream}
                  {sup.dirty_count > 0 && `, with ${sup.dirty_count} uncommitted change${sup.dirty_count === 1 ? "" : "s"}`}
                  {sup.untracked > 0 && ` and ${sup.untracked} untracked file${sup.untracked === 1 ? "" : "s"}`}.
                </p>
                {sup.file_overlap.length > 0 && (
                  <p className="text-amber-300/90 flex items-start gap-1.5">
                    <AlertTriangle size={12} className="shrink-0 mt-0.5" />
                    <span className="break-words">
                      Both sides changed {sup.file_overlap.slice(0, 5).join(", ")}
                      {sup.file_overlap.length > 5 && ` and ${sup.file_overlap.length - 5} more`} — the sync pauses
                      on any conflict so you can resolve it here.
                    </span>
                  </p>
                )}
                {sup.notes.map((n) => (
                  <p key={n} className="break-words">
                    {n}
                  </p>
                ))}
                {sup.own_commits.length > 0 && (
                  <details>
                    <summary className="cursor-pointer select-none hover:text-zinc-300">
                      Your {sup.own_commits.length} commit{sup.own_commits.length === 1 ? "" : "s"} that stay on top
                    </summary>
                    <ul className="mt-1 space-y-0.5 font-mono">
                      {sup.own_commits.map((c) => (
                        <li key={c.oid} className="truncate">
                          {c.short} {c.subject}
                          {c.touches.length > 0 && (
                            <span className="text-zinc-500"> · moves {c.touches.join(", ")}</span>
                          )}
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
              </div>
            )}

            {plan.submodules.length > 0 && (
              <div className="overflow-x-auto">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="text-zinc-500 text-left">
                      <th className="pr-3 pb-1 font-medium">Submodule</th>
                      <th className="pr-3 pb-1 font-medium">On</th>
                      <th className="pb-1 font-medium">What happens</th>
                    </tr>
                  </thead>
                  <tbody>
                    {plan.submodules.map((s) => (
                      <tr key={s.path} className="align-top border-t border-zinc-800/60">
                        <td className="pr-3 py-1 font-mono text-zinc-200 whitespace-nowrap">{s.path}</td>
                        <td className="pr-3 py-1 text-zinc-400 whitespace-nowrap">
                          {s.detached ? "detached" : s.branch ?? "—"}
                        </td>
                        <td className="py-1">
                          <span className={s.action === "blocked" ? "text-red-300" : "text-zinc-200"}>
                            {actionLabel(s)}
                          </span>
                          <span className="text-zinc-500"> — {s.reason}</span>
                          {s.notes.map((n) => (
                            <p key={n} className="text-zinc-500">
                              {n}
                            </p>
                          ))}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            {plan.skipped.length > 0 && (
              <details className="text-xs text-zinc-500">
                <summary className="cursor-pointer select-none hover:text-zinc-400">
                  {plan.skipped.length} gitlink{plan.skipped.length === 1 ? "" : "s"} left alone
                </summary>
                <ul className="mt-1 space-y-0.5 font-mono">
                  {plan.skipped.map((k) => (
                    <li key={k.path} className="truncate">
                      {k.path}{" "}
                      <span className="font-sans">
                        ({k.why === "unmapped" ? "not in .gitmodules" : "not initialised"})
                      </span>
                    </li>
                  ))}
                </ul>
              </details>
            )}

            {!plan.refusal && !plan.nothing_to_do && (
              <div className="flex flex-wrap items-center justify-between gap-3 pt-1">
                <div className="space-y-1.5">
                  <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
                    <Checkbox
                      checked={stash}
                      onChange={onStash}
                      className="mt-0.5"
                      aria-label="Stash uncommitted changes during the sync"
                    />
                    <span className="leading-relaxed">
                      Stash my {sup.dirty_count} uncommitted change{sup.dirty_count === 1 ? "" : "s"} during the sync
                      <span className="text-zinc-500"> — and put them back afterwards</span>
                    </span>
                  </label>
                  <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
                    <Checkbox
                      checked={bundles}
                      onChange={setBundles}
                      className="mt-0.5"
                      aria-label="Also save bundle backups to a folder"
                    />
                    <span className="leading-relaxed">
                      Also save bundle backups to a folder
                      <span className="text-zinc-500">
                        {" "}
                        — a full copy of every branch in GitSwitch&apos;s data folder (sync-backups), outside the repository
                      </span>
                    </span>
                  </label>
                </div>
                <Button
                  size="sm"
                  variant="primary"
                  disabled={busy || assessing || !plan.can_run}
                  title={plan.can_run ? "Runs exactly the plan above" : "Clear the blockers above first"}
                  onClick={() => onSync(stash, bundles, plan.fingerprint)}
                  className="min-w-[7rem]"
                >
                  <Play size={13} />
                  Sync now
                </Button>
              </div>
            )}
          </div>
        )}
      </div>
    </Card>
  );
}

type PausedProps = {
  sync: SyncSummary;
  status: RepoStatus;
  busy: boolean;
  onContinue: () => void;
  onAbort: () => void;
  onOpenRoot: () => void;
};

/** Replaces the generic operation banner while a sync is paused here or above. */
export function SyncPausedCard({ sync, status, busy, onContinue, onAbort, onOpenRoot }: PausedProps) {
  const rootName = baseName(sync.root);
  if (!sync.is_root) {
    return (
      <Card className="border-amber-500/30 bg-amber-500/5">
        <div className="flex flex-wrap items-center gap-3">
          <AlertTriangle size={16} className="text-amber-400 shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="text-sm text-amber-100">A sync of {rootName} is paused inside this submodule.</p>
            <p className="text-xs text-amber-200/70">
              Resolve and stage the conflicts here, then continue the sync from {rootName}.
            </p>
          </div>
          <Button size="sm" variant="secondary" disabled={busy} onClick={onOpenRoot}>
            Open {rootName}
          </Button>
        </div>
      </Card>
    );
  }
  const where = sync.needs_user?.where_ ?? "";
  const hereConflicts = (where === "superproject" || where === "autostash-conflict") && status.conflicted_count > 0;
  return (
    <Card className="border-amber-500/30 bg-amber-500/5">
      <div className="flex flex-wrap items-center gap-3">
        <AlertTriangle size={16} className="text-amber-400 shrink-0" />
        <div className="min-w-0 flex-1 space-y-1">
          <p className="text-sm text-amber-100">
            Sync paused{where.startsWith("submodule") ? ` inside ${where.split(":")[1] ?? "a submodule"}` : ""}.
          </p>
          <p className="text-xs text-amber-200/70 break-words">
            {sync.needs_user?.hint ?? "Continue when you are ready, or abort to put everything back."}
          </p>
          {sync.needs_user && sync.needs_user.paths.length > 0 && (
            <p className="text-xs font-mono text-amber-200/80 break-all">
              {sync.needs_user.paths.slice(0, 8).join(", ")}
              {sync.needs_user.paths.length > 8 && ` and ${sync.needs_user.paths.length - 8} more`}
            </p>
          )}
        </div>
        <Button
          size="sm"
          variant="primary"
          disabled={busy || hereConflicts}
          title={hereConflicts ? "Resolve and stage every conflicted file first" : "Resumes where it stopped"}
          onClick={onContinue}
        >
          <Play size={13} />
          Continue sync
        </Button>
        <Button
          size="sm"
          variant="secondary"
          disabled={busy}
          title="Puts this repository and every submodule the sync rebased back where they were"
          onClick={onAbort}
        >
          <CircleSlash size={13} />
          Abort sync
        </Button>
      </div>
    </Card>
  );
}

type OutcomeProps = {
  outcome: SyncOutcome;
  busy: boolean;
  onRecordPointers: () => void;
};

const CHECKS: [keyof SyncOutcome["verified"], string][] = [
  ["behind_zero", "the branch is no longer behind"],
  ["own_on_top", "your commits are on top"],
  ["dirty_paths_unchanged", "your uncommitted changes are back"],
  ["untracked_unchanged", "untracked files are untouched"],
  ["submodules_aligned", "every submodule matches the recorded commit"],
  ["no_conflicts_left", "no conflicts remain"],
];

/** What a finished (or paused, or aborted) sync did, repository by repository. */
export function SyncOutcomeView({ outcome, busy, onRecordPointers }: OutcomeProps) {
  const failed = CHECKS.filter(([k]) => !outcome.verified[k]);
  return (
    <div className="space-y-2 text-xs">
      <div className="overflow-x-auto">
        <table className="w-full">
          <thead>
            <tr className="text-zinc-500 text-left">
              <th className="pr-3 pb-1 font-medium">Repository</th>
              <th className="pr-3 pb-1 font-medium">Before → after</th>
              <th className="pb-1 font-medium">What happened</th>
            </tr>
          </thead>
          <tbody>
            <tr className="align-top border-t border-zinc-800/60">
              <td className="pr-3 py-1 text-zinc-200 whitespace-nowrap">this repository</td>
              <td className="pr-3 py-1 font-mono text-zinc-400 whitespace-nowrap">
                {short(outcome.head_before)} → {short(outcome.head_after)}
              </td>
              <td className="py-1 text-zinc-300">
                {outcome.incoming} incoming, {outcome.own_kept.length} of yours kept
                {outcome.own_dropped.length > 0 && `, ${outcome.own_dropped.length} dropped`}
                {outcome.stashed && " · uncommitted changes stashed and re-applied"}
              </td>
            </tr>
            {outcome.submodules.map((s) => (
              <tr key={s.path} className="align-top border-t border-zinc-800/60">
                <td className="pr-3 py-1 font-mono text-zinc-200 whitespace-nowrap">{s.path}</td>
                <td className="pr-3 py-1 font-mono text-zinc-400 whitespace-nowrap">
                  {short(s.head_before)} → {short(s.head_after)}
                </td>
                <td className={`py-1 ${s.ok ? "text-zinc-300" : "text-red-300"}`}>
                  {s.action}
                  {s.note && <span className="text-zinc-500"> — {s.note}</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {outcome.own_dropped.length > 0 && (
        <p className="text-zinc-400 break-words">
          Dropped because upstream already had them:{" "}
          {outcome.own_dropped.map((c) => c.subject).join("; ")}.
        </p>
      )}

      {failed.length > 0 && (
        <ul className="space-y-0.5" aria-label="Checks that did not pass">
          {failed.map(([k, label]) => (
            <li key={k} className="flex items-start gap-1.5 text-red-300">
              <XCircle size={12} className="shrink-0 mt-0.5" />
              <span>Check failed: {label}.</span>
            </li>
          ))}
        </ul>
      )}
      {failed.length === 0 && outcome.phase === "done" && (
        <p className="flex items-center gap-1.5 text-emerald-300/90">
          <CheckCircle2 size={12} className="shrink-0" />
          All {CHECKS.length} checks passed.
        </p>
      )}

      {outcome.stash_left && (
        <p className="text-amber-300/90 break-words">
          A stash entry the sync made could not be re-applied and is still in <code>git stash list</code>.
          Your changes are safe in it; apply it by hand when ready.
        </p>
      )}

      {outcome.unrecorded_pointers.length > 0 && (
        <div className="flex flex-wrap items-center gap-2">
          <p className="text-zinc-300">
            {outcome.unrecorded_pointers.join(", ")}{" "}
            {outcome.unrecorded_pointers.length === 1 ? "is" : "are"} checked out ahead of what this branch records.
          </p>
          <Button size="sm" variant="secondary" disabled={busy} onClick={onRecordPointers}>
            Record submodule pointers
          </Button>
        </div>
      )}

      {outcome.backups.length > 0 && (
        <details>
          <summary className="cursor-pointer select-none text-zinc-500 hover:text-zinc-400">
            Backups: {outcome.backups.length} branch{outcome.backups.length === 1 ? "" : "es"}
            {outcome.bundles.length > 0 && `, ${outcome.bundles.length} bundle${outcome.bundles.length === 1 ? "" : "s"}`}
            {" "}— how to undo
          </summary>
          <ul className="mt-1 space-y-1">
            {outcome.backups.map((b) => (
              <li key={b.repo + b.branch} className="font-mono text-zinc-500 break-all">
                {b.recovery}
              </li>
            ))}
            {outcome.bundles.map((p) => (
              <li key={p} className="font-mono text-zinc-500 break-all">
                bundle: {p}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
