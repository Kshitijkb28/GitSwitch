import { useState } from "react";
import {
  Lock,
  LockOpen,
  ShieldAlert,
  AlertTriangle,
  Wrench,
  Loader2,
  KeyRound,
  Terminal,
  CheckCircle2,
  XCircle,
} from "lucide-react";
import { Button } from "../Button";
import * as api from "../../lib/api";
import type { PushState, PushMode, PushModeResult, JobOutcomeKind } from "../../lib/api";
import { LockDisclosure } from "./LockDisclosure";

interface Props {
  repoPath: string;
  push: PushState;
  onChanged: (p: PushState) => void;
}

type Segment = { value: PushMode; label: string; icon: typeof Lock; what: string };

const SEGMENTS: Segment[] = [
  {
    value: "allowed",
    label: "Allowed",
    icon: LockOpen,
    what: "Pushes go through. Nothing is written into the repository.",
  },
  {
    value: "guardrail",
    label: "Guard-rail",
    icon: ShieldAlert,
    what: "Blocks git push here and in your terminal through the repository's own config and a pre-push hook. Anyone with a terminal can turn it off with one command — it stops accidents, not intent.",
  },
  {
    value: "locked",
    label: "Locked",
    icon: Lock,
    what: "The same block, written by an administrator into system-wide git config. Turning it off — here or anywhere on this machine — needs the administrator password.",
  },
];

/** Which segment is on, from the measured state. */
export function currentMode(push: PushState): PushMode {
  if (push.lock.locked) return "locked";
  if (push.repo_blocked) return "guardrail";
  return "allowed";
}

/** What the operating system will do when the job runs. */
export function promptSentence(platform: string | null): string {
  switch (platform) {
    case "macos":
      return "macOS will ask for an administrator name and password.";
    case "linux":
      return "Your system will ask for the administrator password.";
    case "windows":
      return "Windows will ask you to confirm the change (UAC).";
    default:
      return "The operating system will ask for administrator rights.";
  }
}

const OUTCOME_STYLE: Record<JobOutcomeKind, string> = {
  applied: "bg-emerald-500/10 border-emerald-500/30 text-emerald-200/90",
  cancelled: "bg-zinc-700/40 border-zinc-600/50 text-zinc-300",
  denied: "bg-amber-500/10 border-amber-500/30 text-amber-200/90",
  "no-agent": "bg-amber-500/10 border-amber-500/30 text-amber-200/90",
  "manual-required": "bg-amber-500/10 border-amber-500/30 text-amber-200/90",
  busy: "bg-zinc-700/40 border-zinc-600/50 text-zinc-300",
  failed: "bg-red-500/10 border-red-500/30 text-red-200/90",
  refused: "bg-red-500/10 border-red-500/30 text-red-200/90",
  unsupported: "bg-zinc-700/40 border-zinc-600/50 text-zinc-300",
};

/**
 * Three states, not a checkbox: the guard-rail is the user's own setting, the
 * lock is an administrator's. Switching to or from Locked shows what will be
 * written and which prompt will appear before anything runs, and every outcome
 * of that prompt — cancelled included — comes back as a sentence.
 */
export function PushAccessCard({ repoPath, push, onChanged }: Props) {
  const [busy, setBusy] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const [pending, setPending] = useState<PushMode | null>(null);
  const [result, setResult] = useState<PushModeResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const mode = currentMode(push);
  const lock = push.lock;
  const needsPrompt = (target: PushMode) => target === "locked" || mode === "locked";

  const choose = (target: PushMode) => {
    if (target === mode || busy) return;
    setError(null);
    setResult(null);
    if (needsPrompt(target)) {
      setPending(target);
    } else {
      void apply(target);
    }
  };

  const apply = async (target: PushMode) => {
    setBusy(true);
    setWaiting(needsPrompt(target));
    setError(null);
    try {
      const r = await api.changesSetPushMode(repoPath, target);
      setResult(r);
      if (r.state) onChanged(r.state);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      setWaiting(false);
      setPending(null);
    }
  };

  const repairLock = async () => {
    setBusy(true);
    setWaiting(lock.needs_elevation);
    setError(null);
    try {
      const r = await api.changesRepairPushLock(repoPath);
      setResult(r);
      if (r.state) onChanged(r.state);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      setWaiting(false);
    }
  };

  const repairGuardrail = async () => {
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

  const finishManual = async () => {
    if (!result?.job_nonce) return;
    setBusy(true);
    try {
      const o = await api.pushLockFinishManual(result.job_nonce);
      setResult({ ...result, outcome: o.outcome, message: o.message, command: o.outcome === "applied" ? null : result.command });
      if (o.outcome === "applied") onChanged(await api.changesPushState(repoPath));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const HeaderIcon = mode === "locked" ? Lock : mode === "guardrail" ? ShieldAlert : LockOpen;
  const headerColor = mode === "locked" ? "text-amber-400" : mode === "guardrail" ? "text-amber-300/80" : "text-zinc-400";
  const shown = pending ?? mode;
  const shownSegment = SEGMENTS.find((s) => s.value === shown) ?? SEGMENTS[0];

  const segmentDisabled = (s: Segment): string | null => {
    if (busy) return null;
    if (s.value === "locked" && !lock.supported) return "Locking is not available on this operating system yet.";
    if (s.value !== "locked" && push.profile_blocked && mode !== "locked") {
      return "The profile blocks pushes for this folder; change it in the profile.";
    }
    return null;
  };

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-3 space-y-2.5">
      <div className="flex items-center gap-2">
        <HeaderIcon size={15} className={`${headerColor} shrink-0`} />
        <h3 className="text-sm font-medium text-zinc-200">Push access</h3>
        {busy && <Loader2 size={13} className="animate-spin text-zinc-500" />}
      </div>

      <p className="text-xs text-zinc-400 leading-relaxed">{push.reason}</p>

      <div className="flex rounded-lg border border-zinc-700 overflow-hidden w-fit max-w-full" role="radiogroup" aria-label="Push access">
        {SEGMENTS.map((s) => {
          const Icon = s.icon;
          const active = mode === s.value;
          const why = segmentDisabled(s);
          const disabled = busy || why !== null;
          return (
            <button
              key={s.value}
              role="radio"
              aria-checked={active}
              aria-label={s.label}
              disabled={disabled}
              onClick={() => choose(s.value)}
              title={why ?? s.what}
              className={`flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium transition-colors border-r border-zinc-700 last:border-r-0 ${
                active
                  ? s.value === "locked"
                    ? "bg-amber-600 text-white"
                    : "bg-emerald-600 text-white"
                  : disabled
                    ? "bg-zinc-800 text-zinc-600 cursor-not-allowed"
                    : "bg-zinc-800 text-zinc-300 hover:bg-zinc-700 cursor-pointer"
              }`}
            >
              <Icon size={13} className="shrink-0" />
              <span>{s.label}</span>
            </button>
          );
        })}
      </div>
      <p className="text-xs text-zinc-500 leading-relaxed">{shownSegment.what}</p>

      {pending && !waiting && (
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-2.5 py-2 space-y-2">
          <div className="flex items-start gap-2">
            <KeyRound size={13} className="shrink-0 mt-0.5 text-amber-400" />
            <div className="min-w-0 flex-1 space-y-1.5 text-xs text-amber-100/90 leading-relaxed">
              {pending === "locked" ? (
                <>
                  <p>
                    This writes a push rule for{" "}
                    <span className="font-mono break-all">{repoPath}</span> into system-wide git
                    config that only an administrator can change, and re-applies the repository's own
                    block as a mirror. {promptSentence(lock.platform)}
                  </p>
                  {(!lock.helper_installed || lock.helper === "outdated" || lock.helper === "untrusted") && (
                    <p>
                      {!lock.helper_installed
                        ? "First use: GitSwitch installs its lock helper"
                        : lock.helper === "outdated"
                          ? "This GitSwitch ships a newer lock helper, which is installed first"
                          : "The installed lock helper is not the one the registry vouches for, so it is replaced first"}
                      {lock.bundled_helper_sha256 ? (
                        <>
                          {" "}(SHA-256 <span className="font-mono">{lock.bundled_helper_sha256.slice(0, 12)}…</span>)
                        </>
                      ) : null}
                      . This copies a small program from the app into a system folder that only an
                      administrator can change; from then on only that copy runs with administrator
                      rights. It is the same trust you give any app you grant administrator rights to.
                    </p>
                  )}
                </>
              ) : (
                <p>
                  Unlocking removes the administrator-owned rule for this repository
                  {pending === "guardrail" ? " and leaves the guard-rail on" : ""}. {promptSentence(lock.platform)}
                </p>
              )}
            </div>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            <Button size="sm" variant="primary" disabled={busy} onClick={() => void apply(pending)}>
              <KeyRound size={12} />
              {pending === "locked" ? "Lock (administrator prompt)" : "Unlock (administrator prompt)"}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setPending(null)}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {waiting && (
        <div className="flex items-center gap-2 rounded-lg border border-zinc-700 bg-zinc-800/60 px-2.5 py-2 text-xs text-zinc-300">
          <Loader2 size={13} className="animate-spin text-zinc-400 shrink-0" />
          Waiting for the administrator prompt…
        </div>
      )}

      {result && (
        <div className={`rounded-lg border px-2.5 py-2 space-y-2 ${OUTCOME_STYLE[result.outcome] ?? OUTCOME_STYLE.failed}`}>
          <div className="flex items-start gap-2 text-xs leading-relaxed">
            {result.outcome === "applied" ? (
              <CheckCircle2 size={13} className="shrink-0 mt-0.5" />
            ) : result.outcome === "cancelled" ? (
              <XCircle size={13} className="shrink-0 mt-0.5" />
            ) : (
              <AlertTriangle size={13} className="shrink-0 mt-0.5" />
            )}
            <span className="min-w-0 break-words">
              {result.outcome === "cancelled" ? "Cancelled. " : result.outcome === "denied" ? "Not allowed. " : ""}
              {result.message}
            </span>
          </div>
          {result.outcome === "manual-required" && result.command && (
            <div className="space-y-1.5">
              <pre className="text-[11px] font-mono bg-zinc-900 border border-zinc-700 rounded-md px-2 py-1.5 overflow-x-auto whitespace-pre-wrap break-all text-zinc-200">
                {result.command}
              </pre>
              <Button size="sm" variant="secondary" disabled={busy} onClick={() => void finishManual()}>
                <Terminal size={12} />
                I ran it
              </Button>
            </div>
          )}
        </div>
      )}

      {mode === "locked" && (
        <div className="space-y-2">
          {lock.needs_elevation && (
            <div className="flex items-start gap-2 rounded-lg bg-red-500/10 border border-red-500/30 px-2.5 py-2">
              <AlertTriangle size={13} className="shrink-0 mt-0.5 text-red-400" />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-red-200/90 leading-relaxed">
                  Part of the lock's administrator-owned setup is not in place ({lock.drift.filter((d) => !d.startsWith("mirror-")).join(", ")}).
                  Repairing needs the administrator prompt.
                </p>
                <Button size="sm" variant="secondary" className="mt-1.5" disabled={busy} onClick={() => void repairLock()}>
                  <Wrench size={12} />
                  Repair (administrator prompt)
                </Button>
              </div>
            </div>
          )}
          {!lock.needs_elevation && lock.helper === "outdated" && (
            <div className="flex items-start gap-2 rounded-lg bg-zinc-700/40 border border-zinc-600/50 px-2.5 py-2">
              <Wrench size={13} className="shrink-0 mt-0.5 text-zinc-400" />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-zinc-300 leading-relaxed">A newer lock helper ships with this GitSwitch.</p>
                <Button size="sm" variant="ghost" className="mt-1" disabled={busy} onClick={() => void repairLock()}>
                  Upgrade (administrator prompt)
                </Button>
              </div>
            </div>
          )}
          <LockDisclosure lock={lock} />
          {lock.locked_at && (
            <p className="text-xs text-zinc-600">Locked {lock.locked_at.replace("T", " ").replace("Z", " UTC")}.</p>
          )}
        </div>
      )}

      {mode !== "locked" && push.needs_repair && (
        <div className="flex items-start gap-2 rounded-lg bg-amber-500/10 border border-amber-500/30 px-2.5 py-2">
          <AlertTriangle size={13} className="shrink-0 mt-0.5 text-amber-400" />
          <div className="min-w-0 flex-1">
            <p className="text-xs text-amber-200/90 leading-relaxed">
              The block isn't fully in place — usually because a remote was added after it was
              turned on.
            </p>
            <Button size="sm" variant="secondary" className="mt-1.5" disabled={busy} onClick={() => void repairGuardrail()}>
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
              <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${r.rewritten ? "bg-emerald-400" : "bg-amber-400"}`} />
              <span className="text-zinc-400 shrink-0">{r.name}</span>
              <span className="text-zinc-600 truncate min-w-0 font-mono">
                {r.rewritten ? "rewritten" : r.has_explicit_pushurl ? "explicit pushurl — hook only" : "not rewritten"}
              </span>
            </li>
          ))}
        </ul>
      )}

      {mode !== "locked" && push.gaps.length > 0 && (
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
