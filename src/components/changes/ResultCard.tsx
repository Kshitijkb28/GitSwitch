import { CheckCircle2, XCircle } from "lucide-react";
import { Card } from "../Card";
import { Button } from "../Button";
import { SyncOutcomeView } from "./SyncCard";
import type { OpResult } from "../../lib/api";

type Props = {
  result: OpResult;
  busy: boolean;
  onDismiss: () => void;
  onRecordPointers?: () => void;
  /** Rendered after the guidance: e.g. "Open in Changes" / "Open in History". */
  extraAction?: { label: string; onClick: () => void; primary?: boolean };
};

/** Undo/drop counts for a tree or stash outcome, in one sentence. */
function treeSentence(t: NonNullable<OpResult["tree"]>): string | null {
  const parts: string[] = [];
  if (t.backup) parts.push(`Backup branch ${t.backup.branch} keeps ${t.backup.oid.slice(0, 7)}`);
  if ((t.dropped_total ?? 0) > 0) {
    parts.push(`${t.dropped_total} commit${t.dropped_total === 1 ? "" : "s"} left ${t.branch_before ?? "the branch"}`);
  }
  if (t.stash) parts.push(`uncommitted changes saved in ${t.stash.ref}`);
  if (t.deleted_untracked > 0) parts.push(`${t.deleted_untracked} new file${t.deleted_untracked === 1 ? "" : "s"} deleted`);
  return parts.length ? parts.join(" · ") : null;
}

/** The outcome of the last operation: headline, detail, advice, undo lines. */
export function ResultCard({ result, busy, onDismiss, onRecordPointers, extraAction }: Props) {
  const tree = result.tree;
  const stash = result.stash;
  const treeLine = tree ? treeSentence(tree) : null;
  return (
    <Card className={result.ok ? "border-emerald-500/30 bg-emerald-500/5" : "border-red-500/30 bg-red-500/5"}>
      <div className="flex items-start gap-2">
        {result.ok ? (
          <CheckCircle2 size={16} className="text-emerald-400 shrink-0 mt-0.5" />
        ) : (
          <XCircle size={16} className="text-red-400 shrink-0 mt-0.5" />
        )}
        <div className="min-w-0 flex-1 space-y-1">
          <p className={`text-sm ${result.ok ? "text-emerald-100" : "text-red-100"}`}>{result.headline}</p>
          {result.detail && <p className="text-xs text-zinc-400 break-words">{result.detail}</p>}
          {result.advice?.guidance && (
            <p className="text-xs text-zinc-300 break-words">{result.advice.guidance}</p>
          )}
          {extraAction && (
            <div>
              <Button size="sm" variant={extraAction.primary ? "primary" : "secondary"} onClick={extraAction.onClick} disabled={busy}>
                {extraAction.label}
              </Button>
            </div>
          )}
          {result.pull?.recovery && (
            <p className="text-xs text-zinc-500 font-mono break-all">undo: {result.pull.recovery}</p>
          )}
          {result.pull?.autostash_conflict && (
            <p className="text-xs text-amber-300/90 break-words">
              The rebase finished, but your stashed changes could not be re-applied cleanly. They are still in{" "}
              {result.pull.autostash_left ?? "the stash"}; resolve the conflicted files below, or discard them and pop the stash again.
            </p>
          )}
          {treeLine && <p className="text-xs text-zinc-400 break-words">{treeLine}.</p>}
          {tree && (tree.dropped ?? []).length > 0 && (
            <p className="text-xs text-zinc-500 break-words">
              Left the branch: {tree.dropped.map((c) => `${c.short} ${c.subject}`).join("; ")}
              {tree.dropped_total > tree.dropped.length && ` and ${tree.dropped_total - tree.dropped.length} more`}.
            </p>
          )}
          {tree && (tree.submodule_mismatch ?? []).length > 0 && (
            <p className="text-xs text-amber-300/90 break-words">
              {tree.submodule_mismatch.join(", ")} {tree.submodule_mismatch.length === 1 ? "sits" : "sit"} at a commit other than what
              this branch records. Update submodules to align {tree.submodule_mismatch.length === 1 ? "it" : "them"}; their own changes are untouched.
            </p>
          )}
          {tree?.mapping_note && <p className="text-xs text-zinc-500 break-words">{tree.mapping_note}</p>}
          {tree?.recovery && (
            <p className="text-xs text-zinc-500 font-mono break-all">undo: {tree.recovery}</p>
          )}
          {stash && (stash.conflicts ?? []).length > 0 && (
            <p className="text-xs text-amber-300/90 break-words">
              Conflicts in {stash.conflicts.length} file{stash.conflicts.length === 1 ? "" : "s"} — resolve them below
              {stash.kept ? "; the stash is still in the list" : ""}.
            </p>
          )}
          {stash && (stash.not_stashed_submodules ?? []).length > 0 && (
            <p className="text-xs text-zinc-500 break-words">
              Changes inside {stash.not_stashed_submodules.join(", ")} were not stashed — a stash never includes submodules.
            </p>
          )}
          {stash?.recovery && (
            <p className="text-xs text-zinc-500 font-mono break-all">undo: {stash.recovery}</p>
          )}
          {result.submodules && result.submodules.listed > 0 && (
            <p className="text-xs text-zinc-400">
              Submodules: {result.submodules.downloaded} of {result.submodules.listed} up to date
              {result.submodules.failed_paths.length > 0 && ` — failed: ${result.submodules.failed_paths.join(", ")}`}.
            </p>
          )}
          {result.lfs && result.lfs.uses_lfs && (
            <p className="text-xs text-zinc-400">Git LFS: {result.lfs.summary}</p>
          )}
          {result.sync && (
            <SyncOutcomeView outcome={result.sync} busy={busy} onRecordPointers={onRecordPointers ?? (() => {})} />
          )}
          {result.advice?.git_said && (
            <details>
              <summary className="text-xs text-zinc-500 cursor-pointer hover:text-zinc-400 select-none">
                what git said
              </summary>
              <pre className="mt-1 text-xs text-zinc-500 whitespace-pre-wrap break-words">{result.advice.git_said}</pre>
            </details>
          )}
        </div>
        <button onClick={onDismiss} className="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer shrink-0">
          dismiss
        </button>
      </div>
    </Card>
  );
}
