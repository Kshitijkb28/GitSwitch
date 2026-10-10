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
  /** Checks out, in these submodules, the commits the repository records ("Align submodules"). */
  onAlignSubmodules?: (paths: string[]) => void;
  /** Rendered after the guidance: e.g. "Open in Changes" / "Open in History". */
  extraAction?: { label: string; onClick: () => void; primary?: boolean };
  /** Several at once (a push refused for more than one submodule); wins over `extraAction`. */
  extraActions?: { label: string; onClick: () => void; primary?: boolean }[];
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
export function ResultCard({ result, busy, onDismiss, onRecordPointers, onAlignSubmodules, extraAction, extraActions }: Props) {
  const actions = extraActions ?? (extraAction ? [extraAction] : []);
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
          {/* A failed result carries the guidance in `detail` too (for readers without advice) — say it once. */}
          {result.advice?.guidance && result.advice.guidance !== result.detail && (
            <p className="text-xs text-zinc-300 break-words">{result.advice.guidance}</p>
          )}
          {actions.length > 0 && (
            <div className="flex flex-wrap gap-2">
              {actions.map((a) => (
                <Button key={a.label} size="sm" variant={a.primary ? "primary" : "secondary"} onClick={a.onClick} disabled={busy}>
                  {a.label}
                </Button>
              ))}
            </div>
          )}
          {result.pull?.recovery && (
            <p className="text-xs text-zinc-500 font-mono break-all">undo: {result.pull.recovery}</p>
          )}
          {(result.pull?.submodules_moved ?? 0) > 0 && (
            <p className="text-xs text-sky-300/90 break-words">
              {result.pull?.submodules_moved} submodule pointer{result.pull?.submodules_moved === 1 ? "" : "s"} moved — this repository now
              records other commits than are checked out. Update aligns them.
            </p>
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
            <div className="space-y-1">
              <p className="text-xs text-amber-300/90 break-words">
                {tree.submodule_mismatch.join(", ")} {tree.submodule_mismatch.length === 1 ? "sits" : "sit"} at a commit other than what
                this repository records. Align submodules checks out the commit{tree.submodule_mismatch.length === 1 ? "" : "s"} it
                records; {tree.submodule_mismatch.length === 1 ? "its" : "their"} own branches and changes are untouched.
              </p>
              {onAlignSubmodules && (
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={busy}
                  onClick={() => onAlignSubmodules(tree.submodule_mismatch)}
                  title={`Checks out, inside ${tree.submodule_mismatch.join(", ")}, the commit this repository records. Nothing is fetched; their branches keep their commits.`}
                >
                  Align submodules
                </Button>
              )}
            </div>
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
