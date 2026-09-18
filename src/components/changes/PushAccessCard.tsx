import { useState } from "react";
import { Lock, LockOpen, AlertTriangle, Wrench, Loader2 } from "lucide-react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import * as api from "../../lib/api";
import type { PushState } from "../../lib/api";

interface Props {
  repoPath: string;
  push: PushState;
  onChanged: (p: PushState) => void;
}

/**
 * Turning this on writes two things into the repo: a `pushInsteadOf` rewrite in
 * its own .git/config, and a pre-push hook. Between them, `git push` fails in
 * the terminal as well as here — but neither is a lock, and the card says so
 * rather than implying more than it delivers.
 */
export function PushAccessCard({ repoPath, push, onChanged }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggle = async (blocked: boolean) => {
    setBusy(true);
    setError(null);
    try {
      onChanged(await api.changesSetPushBlocked(repoPath, blocked));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const repair = async () => {
    setBusy(true);
    setError(null);
    try {
      onChanged(await api.changesRepairPushBlock(repoPath));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-3 space-y-2.5">
      <div className="flex items-center gap-2">
        {push.blocked ? (
          <Lock size={15} className="text-amber-400 shrink-0" />
        ) : (
          <LockOpen size={15} className="text-zinc-400 shrink-0" />
        )}
        <h3 className="text-sm font-medium text-zinc-200">Push access</h3>
        {busy && <Loader2 size={13} className="animate-spin text-zinc-500" />}
      </div>

      <p className="text-xs text-zinc-400 leading-relaxed">{push.reason}</p>

      <label
        className={`flex items-center gap-2 text-xs ${
          push.profile_blocked ? "text-zinc-600 cursor-not-allowed" : "text-zinc-300 cursor-pointer"
        }`}
        title={
          push.profile_blocked
            ? "The profile already blocks pushes, and a repo can't re-allow what a profile blocks"
            : "Blocks git push from this folder, here and in your terminal"
        }
      >
        <Checkbox
          checked={push.repo_blocked}
          onChange={toggle}
          disabled={busy || push.profile_blocked}
          aria-label="Block pushes from this repository"
        />
        Block pushes from this repository
      </label>

      {push.needs_repair && (
        <div className="flex items-start gap-2 rounded-lg bg-amber-500/10 border border-amber-500/30 px-2.5 py-2">
          <AlertTriangle size={13} className="shrink-0 mt-0.5 text-amber-400" />
          <div className="min-w-0 flex-1">
            <p className="text-xs text-amber-200/90 leading-relaxed">
              The block isn't fully in place — usually because a remote was added after it
              was turned on.
            </p>
            <Button size="sm" variant="secondary" className="mt-1.5" disabled={busy} onClick={repair}>
              <Wrench size={12} />
              Re-apply
            </Button>
          </div>
        </div>
      )}

      {push.blocked && push.remotes.length > 0 && (
        <ul className="space-y-1">
          {push.remotes.map((r) => (
            <li key={r.name} className="flex items-center gap-2 text-xs min-w-0">
              <span
                className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                  r.rewritten ? "bg-emerald-400" : "bg-amber-400"
                }`}
              />
              <span className="text-zinc-400 shrink-0">{r.name}</span>
              <span className="text-zinc-600 truncate font-mono">
                {r.rewritten ? "rewritten" : "not rewritten"}
              </span>
            </li>
          ))}
        </ul>
      )}

      {push.gaps.length > 0 && (
        <details className="group">
          <summary className="text-xs text-zinc-500 cursor-pointer hover:text-zinc-400 select-none">
            What this doesn't stop
          </summary>
          <ul className="mt-1.5 space-y-1 pl-3">
            {push.gaps.map((g, i) => (
              <li key={i} className="text-xs text-zinc-500 leading-relaxed list-disc">
                {g}
              </li>
            ))}
          </ul>
        </details>
      )}

      {error && <p className="text-xs text-red-400 break-words">{error}</p>}
    </div>
  );
}
