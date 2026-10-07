import { useSyncExternalStore } from "react";
import * as api from "./api";
import type { CloneProgress, OpResult } from "./api";

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
export type GitJobKind = "pull" | "push" | "fetch" | "submodules" | "lfs" | "sync" | "assess";

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
  /** What git is doing right now, for a download that reports it (submodules). */
  progress?: CloneProgress;
};

/** The kinds whose backend records progress under the repository's path. */
const REPORTS_PROGRESS: ReadonlySet<GitJobKind> = new Set(["submodules"]);

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
  // A superproject and its submodules share files: a sync of the parent must
  // not race a pull inside a submodule, and vice versa.
  for (const j of jobs.values()) {
    if (j.running && (repo.startsWith(j.repo + "/") || j.repo.startsWith(repo + "/"))) return undefined;
  }

  jobs.set(repo, { repo, kind, label, startedAt: Date.now(), running: true });
  emit();
  // Progress, read from the backend while the job runs — the same line the
  // Clone page shows: phase, percentage, which submodule, bytes and rate.
  const poll = REPORTS_PROGRESS.has(kind)
    ? window.setInterval(() => {
        api
          .cloneProgress(repo)
          .then((p) => {
            const j = jobs.get(repo);
            if (!j?.running || !p) return;
            jobs.set(repo, { ...j, progress: p });
            emit();
          })
          .catch(() => {});
      }, 500)
    : null;

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
  } finally {
    if (poll !== null) window.clearInterval(poll);
  }
}

/** "1.2 GB" from a host's kilobytes. */
export function formatKb(kb: number): string {
  if (kb < 1024) return `${kb} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
  return `${(mb / 1024).toFixed(mb / 1024 < 10 ? 1 : 0)} GB`;
}

/** "about 10 min left" — an estimate, and worded as one. */
export function timeLeftLabel(secs: number): string {
  if (secs < 60) return "less than a minute left";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `about ${mins} min left`;
  return `about ${Math.floor(mins / 60)} h ${mins % 60} min left`;
}

/** "10.00 MiB of about 1.2 GB" when the host said how big it is, else "10.00 MiB". */
export function arrivedLabel(p: CloneProgress): string | null {
  if (!p.bytes) return null;
  return p.estimate_kb ? `${p.bytes} of about ${formatKb(p.estimate_kb)}` : p.bytes;
}

/** "Submodules 37% · 3 of 8 · libs/sub · 10.00 MiB of about 1.2 GB · 2.00 MiB/s · about 10 min left" */
export function progressLine(p: CloneProgress): string {
  const parts = [`${p.phase}${p.percent !== null ? ` ${p.percent}%` : ""}`];
  if (p.done !== null && p.total !== null) parts.push(`${p.done.toLocaleString()} of ${p.total.toLocaleString()}`);
  if (p.detail) parts.push(p.detail);
  const arrived = arrivedLabel(p);
  if (arrived) parts.push(arrived);
  if (p.rate) parts.push(p.rate);
  if (typeof p.eta_secs === "number") parts.push(timeLeftLabel(p.eta_secs));
  return parts.join(" · ");
}

/** "3s" / "1m 04s" — shown while a job runs so a slow network is visible. */
export function elapsedLabel(since: number, now: number = Date.now()): string {
  const secs = Math.max(0, Math.floor((now - since) / 1000));
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${String(s).padStart(2, "0")}s`;
}
