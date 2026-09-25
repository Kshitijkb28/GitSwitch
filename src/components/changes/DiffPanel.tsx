import { useEffect, useState } from "react";
import { X, Loader2, FileWarning } from "lucide-react";
import * as api from "../../lib/api";
import type { ChangeEntry, FileDiff } from "../../lib/api";

interface Props {
  repoPath: string;
  entry: ChangeEntry;
  onClose: () => void;
}

/**
 * The body of a diff: the coloured line list plus its loading, error, empty
 * (binary / nothing to show) and truncated states. Shared by the Changes
 * page's overlay and the History page's inline per-file view, so a hunk looks
 * the same wherever it appears.
 */
export function DiffView({
  diff,
  loading,
  error,
}: {
  diff: FileDiff | null;
  loading: boolean;
  error: string | null;
}) {
  return (
    <>
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
          {diff.lines.length > 0 && (
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
export function DiffPanel({ repoPath, entry, onClose }: Props) {
  const hasStaged = entry.staged !== "." && entry.kind !== "untracked";
  const hasUnstaged = entry.unstaged !== "." || entry.kind === "untracked";
  const [side, setSide] = useState<"staged" | "unstaged">(
    hasUnstaged ? "unstaged" : "staged"
  );
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
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
  }, [repoPath, entry.path, entry.kind, side]);

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
          <button
            onClick={onClose}
            className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
          >
            <X size={18} />
          </button>
        </div>

        <div className="flex-1 overflow-auto">
          <DiffView diff={diff} loading={loading} error={error} />
        </div>
      </div>
    </div>
  );
}
