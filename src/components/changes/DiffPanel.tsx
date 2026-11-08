import { useEffect, useMemo, useState } from "react";
import { X, Loader2, FileWarning, ExternalLink, FolderOpen } from "lucide-react";
import { Button } from "../Button";
import { ConflictView } from "./ConflictView";
import { Segmented } from "../github/shared";
import * as api from "../../lib/api";
import type { ChangeEntry, FileDiff } from "../../lib/api";
import { splitRows, useDiffView, type SplitCell, type SplitRow } from "../../lib/diffView";

interface Props {
  repoPath: string;
  entry: ChangeEntry;
  onClose: () => void;
  /** Open the file in the system's default application, or reveal it. */
  onOpenPath?: (path: string, reveal: boolean) => void;
}

/** Unified or side by side — the one choice every diff in the app follows. */
export function DiffViewToggle() {
  const [mode, setMode] = useDiffView();
  return (
    <span data-diff-toggle>
      <Segmented
        label="How diffs are shown"
        value={mode}
        onChange={setMode}
        options={[
          { value: "unified", label: "Unified" },
          { value: "split", label: "Side by side" },
        ]}
      />
    </span>
  );
}

const CELL: Record<SplitCell["tone"], string> = {
  context: "text-zinc-300",
  del: "bg-red-500/10 text-red-300",
  add: "bg-emerald-500/10 text-emerald-300",
};

/** The old version on the left, the new one on the right, each line with its number. */
function SplitTable({ rows }: { rows: SplitRow[] }) {
  const no = (c: SplitCell | null, tone: string) => (
    <td className={`w-12 px-2 text-right align-top select-none text-zinc-600 ${c ? tone : "bg-zinc-950/40"}`}>{c?.no ?? ""}</td>
  );
  const text = (c: SplitCell | null, side: "old" | "new") => (
    <td data-side={side} className={`px-2 align-top whitespace-pre-wrap break-all ${c ? CELL[c.tone] : "bg-zinc-950/40"}`}>
      {c ? c.text || " " : ""}
    </td>
  );
  return (
    <table className="w-full table-fixed border-collapse text-xs font-mono leading-relaxed" aria-label="Side-by-side diff" data-diff-view="split">
      <colgroup>
        <col className="w-12" />
        <col />
        <col className="w-12" />
        <col />
      </colgroup>
      <tbody>
        {rows.map((r, i) =>
          r.kind === "full" ? (
            <tr key={i} data-split-row="full">
              <td colSpan={4} className={`px-4 whitespace-pre-wrap break-all ${r.tone === "hunk" ? "bg-zinc-800 text-sky-300" : "text-zinc-500"}`}>
                {r.text || " "}
              </td>
            </tr>
          ) : (
            <tr key={i} data-split-row="pair" data-old={r.left?.no ?? ""} data-new={r.right?.no ?? ""}>
              {no(r.left, r.left ? CELL[r.left.tone] : "")}
              {text(r.left, "old")}
              {no(r.right, r.right ? CELL[r.right.tone] : "")}
              {text(r.right, "new")}
            </tr>
          )
        )}
      </tbody>
    </table>
  );
}

/**
 * The body of a diff: the coloured line list plus its loading, error, empty
 * (binary / nothing to show) and truncated states. Shared by the Changes
 * page's overlay, the History page's inline per-file view and a pull
 * request's files, so a hunk looks the same wherever it appears — unified,
 * or side by side as the person chose (`toggle`: the choice is offered here;
 * a list of many diffs offers it once, above them).
 */
export function DiffView({
  diff,
  loading,
  error,
  toggle = true,
}: {
  diff: FileDiff | null;
  loading: boolean;
  error: string | null;
  toggle?: boolean;
}) {
  const [mode] = useDiffView();
  const rows = useMemo(() => (mode === "split" && diff && diff.lines.length > 0 ? splitRows(diff.lines) : null), [mode, diff]);
  return (
    <>
      {toggle && !loading && !error && diff && diff.lines.length > 0 && (
        <div className="flex items-center justify-end gap-2 px-3 py-1.5 border-b border-zinc-800/80">
          {mode === "split" && !rows && <span className="text-[11px] text-zinc-500">This diff can only be shown unified.</span>}
          <DiffViewToggle />
        </div>
      )}
      {loading && (
        <div className="flex items-center gap-2 p-6 text-sm text-zinc-400">
          <Loader2 size={16} className="animate-spin" />
          Loading diff…
        </div>
      )}
      {error && <p className="p-6 text-sm text-red-400 break-words">{error}</p>}
      {!loading && !error && diff && (
        <>
          {diff.empty_reason && (
            <div className="flex items-start gap-2 p-6 text-sm text-zinc-400">
              <FileWarning size={16} className="shrink-0 mt-0.5 text-amber-400" />
              <span>{diff.empty_reason}</span>
            </div>
          )}
          {diff.lines.length > 0 && rows && <SplitTable rows={rows} />}
          {diff.lines.length > 0 && !rows && (
            <pre className="text-xs font-mono leading-relaxed">
              {diff.lines.map((l, i) => (
                <div
                  key={i}
                  className={`px-4 whitespace-pre-wrap break-all ${
                    l.kind === "add"
                      ? "bg-emerald-500/10 text-emerald-300"
                      : l.kind === "del"
                        ? "bg-red-500/10 text-red-300"
                        : l.kind === "hunk"
                          ? "bg-zinc-800 text-sky-300 mt-1"
                          : l.kind === "meta"
                            ? "text-zinc-500"
                            : "text-zinc-300"
                  }`}
                >
                  {l.text || " "}
                </div>
              ))}
            </pre>
          )}
          {diff.truncated && (
            <p className="px-4 py-3 text-xs text-amber-400/90 border-t border-zinc-800">
              Showing the first {diff.lines.length} of {diff.total_lines} lines — open
              the file in your editor to see all of it.
            </p>
          )}
        </>
      )}
    </>
  );
}

/**
 * Read-only. Staged and unstaged versions of a file are shown separately
 * because that is how git sees them: a file can be `MM` — one set of changes in
 * the index and a different set in the worktree — and collapsing the two would
 * misrepresent what a commit is about to contain.
 */
export function DiffPanel({ repoPath, entry, onClose, onOpenPath }: Props) {
  // A conflicted file is shown by its sides, not as a diff against the index.
  // A conflicted gitlink has no sides to split: it keeps the pointer diff.
  const conflicted = entry.kind === "conflicted" && !entry.is_submodule;
  const hasStaged = entry.staged !== "." && entry.kind !== "untracked";
  const hasUnstaged = entry.unstaged !== "." || entry.kind === "untracked";
  const [side, setSide] = useState<"staged" | "unstaged">(
    hasUnstaged ? "unstaged" : "staged"
  );
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (conflicted) {
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .changesFileDiff(repoPath, entry.path, side === "staged", entry.kind === "untracked")
      .then((d) => {
        if (!cancelled) setDiff(d);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath, entry.path, entry.kind, side, conflicted]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={onClose} />
      <div className="relative w-full max-w-5xl max-h-[85vh] flex flex-col rounded-xl border border-zinc-700/50 bg-zinc-900 shadow-2xl">
        <div className="flex items-start justify-between gap-3 p-4 border-b border-zinc-800">
          <div className="min-w-0">
            <p className="font-mono text-sm text-zinc-100 truncate min-w-0">{entry.path}</p>
            {entry.orig_path && (
              <p className="text-xs text-zinc-500 truncate mt-0.5">
                renamed from {entry.orig_path}
              </p>
            )}
            <div className="flex items-center gap-3 mt-1.5 text-xs">
              {diff && !diff.is_binary && (
                <span className="font-mono">
                  <span className="text-emerald-400">+{diff.added}</span>{" "}
                  <span className="text-red-400">−{diff.removed}</span>
                </span>
              )}
              {hasStaged && hasUnstaged && (
                <div className="flex rounded-md border border-zinc-700 overflow-hidden">
                  {(["unstaged", "staged"] as const).map((s) => (
                    <button
                      key={s}
                      onClick={() => setSide(s)}
                      className={`px-2 py-0.5 text-xs cursor-pointer transition-colors ${
                        side === s
                          ? "bg-zinc-700 text-zinc-100"
                          : "bg-transparent text-zinc-400 hover:bg-zinc-800"
                      }`}
                    >
                      {s === "staged" ? "Staged" : "Not staged"}
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>
          <div className="flex items-center gap-1 shrink-0">
            {onOpenPath && (
              <>
                <Button size="sm" variant="ghost" onClick={() => onOpenPath(entry.path, false)} title="Open in your editor — the application your system opens this file with" className="px-1.5">
                  <ExternalLink size={13} />
                  Open in editor
                </Button>
                <Button size="sm" variant="ghost" onClick={() => onOpenPath(entry.path, true)} title="Reveal in the file manager" className="px-1.5">
                  <FolderOpen size={13} />
                  Reveal
                </Button>
              </>
            )}
            <button
              onClick={onClose}
              aria-label="Close"
              className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
            >
              <X size={18} />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-auto">
          {conflicted ? <ConflictView repoPath={repoPath} path={entry.path} /> : <DiffView diff={diff} loading={loading} error={error} />}
        </div>
      </div>
    </div>
  );
}
