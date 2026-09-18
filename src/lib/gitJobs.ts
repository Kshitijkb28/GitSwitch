import { useSyncExternalStore } from "react";
import type { OpResult } from "./api";

/**
 * Long git operations keep running while you browse other pages, so their state
 * lives outside React — the same reason the Clone page holds its job here.
 *
 * Without this, leaving the page mid-pull loses the "Pulling…" state, and on
 * return a half-finished pull looks like an idle repo: the page would happily
 * offer to push on top of a merge that is still being applied.
 *
 * One slot per repository, so two repos can be busy at once without either
 * hiding the other.
 */
export type GitJobKind = "pull" | "push" | "fetch" | "submodules";

export type GitJob = {
  repo: string;
  kind: GitJobKind;
  /** Present tense while running: "Pulling (rebase)…" */
  label: string;
  startedAt: number;
  running: boolean;
  result?: OpResult;
  /** A failure to even run the command (not a git refusal, which is in result). */
  error?: string;
};

const jobs = new Map<string, GitJob>();
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function getJob(repo: string): GitJob | undefined {
  return jobs.get(repo);
}

/** The job for one repo, re-rendering the caller whenever it changes. */
export function useGitJob(repo: string): GitJob | undefined {
  return useSyncExternalStore(
    subscribe,
    () => (repo ? jobs.get(repo) : undefined),
    () => undefined
  );
}

/** True while any repo is busy — for a global indicator. */
export function useAnyJobRunning(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => {
      for (const j of jobs.values()) if (j.running) return true;
      return false;
    },
    () => false
  );
}

export function clearJob(repo: string) {
  const job = jobs.get(repo);
  // Never clear a running job: its result would then arrive with nowhere to go.
  if (!job || job.running) return;
  jobs.delete(repo);
  emit();
}

/**
 * Start an operation for `repo`. Refuses to start a second one on the same
 * repo — two concurrent pulls would fight over the index lock.
 */
export async function runJob(
  repo: string,
  kind: GitJobKind,
  label: string,
  fn: () => Promise<OpResult>
): Promise<OpResult | undefined> {
  const existing = jobs.get(repo);
  if (existing?.running) return undefined;

  jobs.set(repo, { repo, kind, label, startedAt: Date.now(), running: true });
  emit();

  try {
    const result = await fn();
    jobs.set(repo, { repo, kind, label, startedAt: Date.now(), running: false, result });
    emit();
    return result;
  } catch (e) {
    jobs.set(repo, {
      repo,
      kind,
      label,
      startedAt: Date.now(),
      running: false,
      error: String(e),
    });
    emit();
    return undefined;
  }
}

/** "3s" / "1m 04s" — shown while a job runs so a slow network is visible. */
export function elapsedLabel(since: number, now: number = Date.now()): string {
  const secs = Math.max(0, Math.floor((now - since) / 1000));
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${String(s).padStart(2, "0")}s`;
}
