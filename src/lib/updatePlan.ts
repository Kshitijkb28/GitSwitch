import { useSyncExternalStore } from "react";
import * as api from "./api";
import type { RepoStatus, SyncPlan } from "./api";
import { runJob } from "./gitJobs";

/**
 * The Update preview for a repository: the plan the backend made, and what it
 * was made for. It lives outside React so leaving the page does not lose a
 * plan that took a fetch to make, and so the strip, the drawer and the tabs
 * all read the same one.
 */
export type UpdatePlanState = {
  assessing: boolean;
  plan: SyncPlan | null;
  error: string | null;
  /** The HEAD and upstream the plan describes; a plan for anything else is stale. */
  forHead: string | null;
  forUpstream: string | null;
  /** A run was refused because upstream moved since the plan: say so once, re-plan. */
  moved: boolean;
};

const EMPTY: UpdatePlanState = { assessing: false, plan: null, error: null, forHead: null, forUpstream: null, moved: false };
const plans = new Map<string, UpdatePlanState>();
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
function set(repo: string, patch: Partial<UpdatePlanState>) {
  plans.set(repo, { ...(plans.get(repo) ?? EMPTY), ...patch });
  emit();
}

export function useUpdatePlan(repo: string): UpdatePlanState {
  return useSyncExternalStore(
    subscribe,
    () => (repo ? plans.get(repo) ?? EMPTY : EMPTY),
    () => EMPTY
  );
}

/** A plan describes one repository at one HEAD with one upstream. Anything else is dropped. */
export function reconcilePlan(repo: string, status: RepoStatus | null) {
  const p = plans.get(repo);
  if (!p || !p.plan || !status) return;
  if (p.forHead !== (status.head_oid ?? null) || p.forUpstream !== (status.upstream ?? null)) {
    plans.delete(repo);
    emit();
  }
}

export function dropPlan(repo: string) {
  if (plans.has(repo)) {
    plans.delete(repo);
    emit();
  }
}

/** The run was refused with `assess-again`: the drawer re-plans and says why. */
export function markMoved(repo: string) {
  set(repo, { moved: true });
}

/** The person has read the new plan (the drawer closed, or ran it). */
export function clearMoved(repo: string) {
  if (plans.get(repo)?.moved) set(repo, { moved: false });
}

/** Why a preview cannot be made right now, in a sentence — or null. */
export function whyNoPreview(status: RepoStatus): string | null {
  if (status.unborn) return "No commits yet.";
  // Before the detached check: a rebase detaches HEAD, and the remedy for that
  // is to finish or abort it, not to switch branches.
  if (status.operation) return `Finish or abort the ${status.operation.kind} first.`;
  if (status.detached) return "Check out a branch first — a detached HEAD has nothing to replay onto.";
  if (!status.upstream) return "This branch has no upstream to update from. Publish it first.";
  if (status.conflicted_count > 0) return "Resolve the conflicts first.";
  return null;
}

/**
 * Assess: fetch (when asked) and plan. It goes through the job slot so nothing
 * else runs on the repository meanwhile and the running banner shows it; the
 * job's result is synthetic and the Changes tab paints nothing for it.
 */
export async function assess(repo: string, status: RepoStatus, stash: boolean, fetch: boolean): Promise<SyncPlan | null> {
  set(repo, { assessing: true, error: null });
  let plan: SyncPlan | null = null;
  const label = fetch ? `Assessing — fetching from ${status.upstream ?? "upstream"}…` : "Assessing…";
  const r = await runJob(repo, "assess", label, async () => {
    try {
      plan = await api.changesSyncPlan(repo, stash, fetch);
      set(repo, { assessing: false, plan, error: null, forHead: status.head_oid ?? null, forUpstream: status.upstream ?? null });
    } catch (e) {
      set(repo, { assessing: false, error: String(e) });
    }
    return { ok: true, headline: "Assessed.", detail: "" };
  });
  if (r === undefined) {
    set(repo, { assessing: false, error: "Something else is still running on this repository — try again when it is done." });
  }
  return plan;
}
