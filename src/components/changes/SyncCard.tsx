import { AlertTriangle, CheckCircle2, CircleSlash, Play, XCircle } from "lucide-react";
import { Button } from "../Button";
import { Card } from "../Card";
import { baseName } from "../../lib/paths";
import type { RepoStatus, SyncOutcome, SyncSummary } from "../../lib/api";

const short = (oid: string | null | undefined) => (oid ? oid.slice(0, 7) : "—");

type PausedProps = {
  sync: SyncSummary;
  status: RepoStatus;
  busy: boolean;
  onContinue: () => void;
  onAbort: () => void;
  onOpenRoot: () => void;
  /** Hands over to where the conflicts are (needs_user.where_): this tab's list, or a submodule as the scope. */
  onHandOff: (where: string) => void;
};

/** Replaces the generic operation banner while an update is paused here or above. */
export function SyncPausedCard({ sync, status, busy, onContinue, onAbort, onOpenRoot, onHandOff }: PausedProps) {
  const rootName = baseName(sync.root);
  if (!sync.is_root) {
    return (
      <Card className="border-amber-500/30 bg-amber-500/5">
        <div className="flex flex-wrap items-center gap-3">
          <AlertTriangle size={16} className="text-amber-400 shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="text-sm text-amber-100">An update of {rootName} is paused inside this submodule.</p>
            <p className="text-xs text-amber-200/70">
              Resolve and stage the conflicts here, then continue the update from {rootName}.
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
  const inSub = where.startsWith("submodule");
  const subPath = inSub ? (where.split(":")[1] ?? "") : "";
  // The backend counts what is still unmerged where the pause is. An older
  // answer has no count: fall back to this repository's own conflicts.
  const unresolved =
    typeof sync.unresolved === "number"
      ? sync.unresolved
      : where === "superproject" || where === "autostash-conflict"
        ? status.conflicted_count
        : 0;
  const hereConflicts = unresolved > 0;
  return (
    <Card className="border-amber-500/30 bg-amber-500/5">
      <div className="flex flex-wrap items-center gap-3">
        <AlertTriangle size={16} className="text-amber-400 shrink-0" />
        <div className="min-w-0 flex-1 space-y-1">
          <p className="text-sm text-amber-100">
            Update paused{inSub ? ` inside ${subPath || "a submodule"}` : ""}.
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
        {!inSub && hereConflicts && status.conflicted_count > 0 && (
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => onHandOff(where)} title="Scrolls to the conflicted files on this tab">
            Go to the conflicts
          </Button>
        )}
        {inSub && subPath && (
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => onHandOff(where)} title="Makes the submodule the scope, where its conflicts can be resolved">
            Open {subPath}
          </Button>
        )}
        <Button
          size="sm"
          variant="primary"
          disabled={busy || hereConflicts}
          title={hereConflicts ? `Resolve and stage every conflicted file first (${unresolved} left)` : "Resumes where it stopped"}
          onClick={onContinue}
        >
          <Play size={13} />
          Continue update
        </Button>
        <Button
          size="sm"
          variant="secondary"
          disabled={busy}
          title="Puts this repository and every submodule the update rebased back where they were"
          onClick={onAbort}
        >
          <CircleSlash size={13} />
          Abort update
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

/** What a finished (or paused, or aborted) update did, repository by repository. */
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
          A stash entry the update made could not be re-applied and is still in <code>git stash list</code>.
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
