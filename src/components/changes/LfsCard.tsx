import { useCallback, useEffect, useState } from "react";
import { HardDriveDownload, Loader2, AlertTriangle, CheckCircle2 } from "lucide-react";
import { Button } from "../Button";
import * as api from "../../lib/api";
import type { LfsStatus } from "../../lib/api";

interface Props {
  repoPath: string;
  busy: boolean;
  /** Bumped by the page after any operation, so the card re-reads. */
  refreshKey: number;
  onPull: () => void;
}

/**
 * A clone made without git-lfs (or with GIT_LFS_SKIP_SMUDGE) looks complete
 * but every large file is a ~130-byte pointer stub. Nothing in `git status`
 * says so, which is why this card exists: it counts the stubs and offers the
 * one command that fixes them.
 */
export function LfsCard({ repoPath, busy, refreshKey, onPull }: Props) {
  const [lfs, setLfs] = useState<LfsStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showPaths, setShowPaths] = useState(false);

  const load = useCallback(() => {
    if (!repoPath) return;
    let cancelled = false;
    setLoading(true);
    api
      .changesLfsStatus(repoPath)
      .then((s) => {
        if (cancelled) return;
        setLfs(s);
        setError(null);
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
  }, [repoPath]);

  useEffect(load, [load, refreshKey]);

  const pointers = lfs?.pointers ?? 0;
  const canPull = !!lfs && lfs.installed && lfs.uses_lfs && pointers > 0;

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-3 space-y-2">
      <div className="flex items-center gap-2">
        <HardDriveDownload size={15} className="text-zinc-400 shrink-0" />
        <h3 className="text-sm font-medium text-zinc-200">Git LFS</h3>
        {loading && <Loader2 size={13} className="animate-spin text-zinc-500" />}
        {lfs && lfs.installed && lfs.uses_lfs && pointers === 0 && (
          <CheckCircle2 size={13} className="text-emerald-400 shrink-0" />
        )}
      </div>

      {error && <p className="text-xs text-red-400 break-words">{error}</p>}

      {lfs && (
        <>
          <p
            className={`text-xs leading-relaxed ${
              !lfs.installed
                ? "text-amber-300/90"
                : pointers > 0
                  ? "text-zinc-200"
                  : "text-zinc-400"
            }`}
          >
            {lfs.summary}
          </p>

          {!lfs.installed && (
            <div className="flex items-start gap-2 rounded-lg bg-amber-500/10 border border-amber-500/30 px-2.5 py-2">
              <AlertTriangle size={13} className="shrink-0 mt-0.5 text-amber-400" />
              <p className="text-xs text-amber-200/90 leading-relaxed">
                Install it with <span className="font-mono">brew install git-lfs</span>, then
                come back here to download the files.
              </p>
            </div>
          )}

          {pointers > 0 && (
            <div>
              <button
                onClick={() => setShowPaths((v) => !v)}
                className="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer"
              >
                {showPaths ? "Hide the files" : `Which ${pointers === 1 ? "file" : "files"}?`}
              </button>
              {showPaths && (
                <ul className="mt-1 max-h-40 overflow-y-auto space-y-0.5">
                  {lfs.pointer_paths.map((p) => (
                    <li key={p} className="text-xs font-mono text-zinc-400 truncate" title={p}>
                      {p}
                    </li>
                  ))}
                  {lfs.more_pointers > 0 && (
                    <li className="text-xs text-zinc-600">…and {lfs.more_pointers} more</li>
                  )}
                </ul>
              )}
            </div>
          )}

          <Button
            size="sm"
            variant={canPull ? "primary" : "secondary"}
            disabled={busy || !canPull}
            onClick={onPull}
            title={
              !lfs.installed
                ? "git-lfs isn't installed"
                : pointers === 0
                  ? "Nothing to download"
                  : "Runs git lfs pull — downloads the real content for every pointer stub"
            }
          >
            <HardDriveDownload size={13} />
            {pointers > 0 ? `Pull LFS files (${pointers})` : "Pull LFS files"}
          </Button>
        </>
      )}
    </div>
  );
}
