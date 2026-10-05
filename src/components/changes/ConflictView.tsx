import { useEffect, useState } from "react";
import { Loader2, FileWarning } from "lucide-react";
import * as api from "../../lib/api";
import type { ConflictView as ConflictViewData } from "../../lib/api";

type Props = {
  repoPath: string;
  path: string;
};

/**
 * A conflicted file, hunk by hunk: what each side wants there, named the way
 * the file list names the sides. Read-only — Keep mine / Take theirs on the
 * row take a whole side, "Open in editor" is for anything finer.
 */
export function ConflictView({ repoPath, path }: Props) {
  const [view, setView] = useState<ConflictViewData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .changesConflictView(repoPath, path)
      .then((v) => {
        if (!cancelled) setView(v);
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
  }, [repoPath, path]);

  if (loading) {
    return (
      <div className="flex items-center gap-2 p-6 text-sm text-zinc-400">
        <Loader2 size={16} className="animate-spin" />
        Reading the conflict…
      </div>
    );
  }
  if (error) return <p className="p-6 text-sm text-red-400 break-words">{error}</p>;
  if (!view) return null;

  if (view.binary) {
    return (
      <div className="flex items-start gap-2 p-6 text-sm text-zinc-400">
        <FileWarning size={16} className="shrink-0 mt-0.5 text-amber-400" />
        <span>A binary file has no lines to compare: Keep mine or Take theirs decides it.</span>
      </div>
    );
  }

  return (
    <div className="p-4 space-y-4 text-xs" aria-label="Conflict view">
      <p className="text-zinc-400 leading-relaxed">
        mine = <span className="text-zinc-200">{view.sides.mine}</span> · theirs = <span className="text-zinc-200">{view.sides.theirs}</span>
        <span className="text-zinc-600">
          {" "}
          · {view.marker_count} conflict{view.marker_count === 1 ? "" : "s"} left in the file
        </span>
      </p>
      {view.marker_count === 0 && (
        <p className="text-emerald-300/90">No conflict markers remain — the file reads as resolved. Mark it resolved on the list.</p>
      )}
      {view.hunks.map((h, i) => (
        <div key={h.line} className="rounded-lg border border-zinc-800 overflow-hidden">
          <div className="px-3 py-1 bg-zinc-800/60 text-zinc-500">
            conflict {i + 1} · line {h.line}
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 divide-y md:divide-y-0 md:divide-x divide-zinc-800">
            <div>
              <div className="px-3 py-1 text-[10px] uppercase tracking-wide text-emerald-300/80 bg-emerald-500/5">mine</div>
              <pre className="px-3 py-2 font-mono whitespace-pre-wrap break-all text-zinc-300">{h.mine.join("\n") || " "}</pre>
            </div>
            <div>
              <div className="px-3 py-1 text-[10px] uppercase tracking-wide text-sky-300/80 bg-sky-500/5">theirs</div>
              <pre className="px-3 py-2 font-mono whitespace-pre-wrap break-all text-zinc-300">{h.theirs.join("\n") || " "}</pre>
            </div>
          </div>
          {h.base.length > 0 && (
            <details className="border-t border-zinc-800">
              <summary className="px-3 py-1 cursor-pointer select-none text-zinc-500 hover:text-zinc-400">what both started from</summary>
              <pre className="px-3 py-2 font-mono whitespace-pre-wrap break-all text-zinc-400">{h.base.join("\n")}</pre>
            </details>
          )}
        </div>
      ))}
      {view.truncated && <p className="text-amber-400/90">Only the first conflicts and lines are shown — open the file in your editor for all of it.</p>}
    </div>
  );
}
