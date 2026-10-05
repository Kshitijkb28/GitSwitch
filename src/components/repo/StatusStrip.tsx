import { RefreshCw, ArrowUp, ArrowDown, GitBranch, Upload, Download, Ban, Radar, Loader2, Box } from "lucide-react";
import { Button } from "../Button";
import { whyDisabled } from "../changes/PullControl";
import { agoLabel } from "../changes/BranchCard";
import type { GitJob } from "../../lib/gitJobs";
import type { PullMode, RemotePeek, RepoStatus, SubmoduleInfo } from "../../lib/api";

/** What the strip's buttons do — supplied by the Changes tab, which owns the operations. */
export type StripActions = {
  fetch: () => void;
  pull: () => void;
  push: () => void;
  branches: () => void;
  refresh: () => void;
};

/**
 * Everything the Changes tab registers with the page: the strip's buttons plus
 * the submodule operations the Submodules tab needs, run by the Changes tab so
 * their results paint there. The superproject is named explicitly: that tab
 * lists `repo.path`'s submodules whatever the scope is. Each settles when the
 * operation has finished, so the caller can re-read.
 */
export type RepoActions = StripActions & {
  initialise: (repo: string, paths: string[]) => Promise<unknown>;
  updateOne: (repo: string, sub: string) => Promise<unknown>;
  pullInside: (repo: string, sub: SubmoduleInfo) => Promise<unknown>;
  recordPointers: (repo: string) => Promise<unknown>;
};

interface Props {
  repoPath: string;
  /** "" for the repository itself, else the submodule path in scope. */
  sub: string;
  status: RepoStatus | null;
  subs: SubmoduleInfo[];
  loading: boolean;
  busy: boolean;
  dirty: boolean;
  pullMode: PullMode;
  pullAutostash: boolean;
  job: GitJob | undefined;
  actions: StripActions | null;
  peek: RemotePeek | null;
  peeking: boolean;
  onPeek: () => void;
  onPeekDismiss: () => void;
}

/**
 * One line that says where you stand, in words, and names ONE next step. The
 * step only points (it is a sentence, not a button): the buttons beside it are
 * the same ones the Changes page had, so nothing here runs git by surprise.
 */
export function glance(status: RepoStatus, subs: SubmoduleInfo[], sub: string): { facts: string; next: string } {
  const facts: string[] = [];
  if (status.detached && status.operation) facts.push(status.operation.label);
  else if (status.detached) facts.push(`detached at ${(status.head_oid ?? "").slice(0, 7) || "?"}`);
  else facts.push(status.upstream ? `${status.branch ?? "no branch"} → ${status.upstream}` : `${status.branch ?? "no branch"} · not published`);
  if (status.ahead > 0 || status.behind > 0) facts.push(`${status.ahead > 0 ? `↑${status.ahead}` : ""}${status.ahead > 0 && status.behind > 0 ? " " : ""}${status.behind > 0 ? `↓${status.behind}` : ""}`);
  const changed = status.staged_count + status.unstaged_count;
  if (changed > 0) facts.push(`${changed} changed`);
  if (status.untracked_count > 0) facts.push(`${status.untracked_count} new`);
  if (status.conflicted_count > 0) facts.push(`${status.conflicted_count} conflicted`);
  if (status.stash_count > 0) facts.push(`${status.stash_count} stash${status.stash_count === 1 ? "" : "es"}`);
  const moved = sub ? [] : subs.filter((m) => m.state === "moved" || m.state === "moved-and-dirty");
  if (moved.length > 0) facts.push(`${moved[0].path}${moved.length > 1 ? ` + ${moved.length - 1} more` : ""} moved`);
  facts.push(agoLabel(status.last_fetch_secs));

  let next: string;
  if (status.sync) next = status.sync.needs_user ? "Resolve the paused update, then continue or abort it" : "Continue or abort the update";
  else if (status.conflicted_count > 0) next = `Resolve ${status.conflicted_count} conflict${status.conflicted_count === 1 ? "" : "s"}`;
  else if (status.operation) next = `Finish or abort the ${status.operation.kind}`;
  else if (status.unborn) next = "Make the first commit";
  else if (!status.upstream && !status.detached) next = "Publish the branch";
  else if (status.behind > 0 || moved.length > 0) next = "Pull the latest";
  else if (status.ahead > 0) next = status.push.blocked ? `Push blocked: ${status.push.reason}` : `Push (${status.ahead})`;
  else if (changed > 0 || status.untracked_count > 0) next = "Commit what you changed";
  else next = "Up to date";
  return { facts: facts.join(" · "), next };
}

export function StatusStrip({
  repoPath,
  sub,
  status,
  subs,
  loading,
  busy,
  dirty,
  pullMode,
  pullAutostash,
  job,
  actions,
  peek,
  peeking,
  onPeek,
  onPeekDismiss,
}: Props) {
  if (!repoPath) return null;
  const pullBlocked = status
    ? whyDisabled(pullMode, status.ahead, status.behind, dirty, pullMode === "rebase" && pullAutostash)
    : "Reading the repository…";
  const opTitle = status?.operation ? `Finish or abort the ${status.operation.kind} first` : null;
  const g = status ? glance(status, subs, sub) : null;
  const running = job?.running ?? false;

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 px-4 py-3 space-y-2" aria-label="Where this repository stands">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0 flex items-center gap-2 flex-wrap">
          {sub ? <Box size={15} className="text-emerald-400 shrink-0" /> : <GitBranch size={15} className="text-zinc-400 shrink-0" />}
          {status ? (
            status.detached && status.operation ? (
              <span className="font-medium text-amber-300">
                {status.operation.label}
                {status.operation.detail && <span className="font-normal text-xs text-amber-200/70"> · {status.operation.detail}</span>}
              </span>
            ) : status.detached ? (
              <span className="font-medium text-amber-300">
                detached HEAD at {(status.head_oid ?? "").slice(0, 7) || "?"} — you&apos;re not on a branch
              </span>
            ) : (
              <span className="font-medium text-zinc-100">{status.branch ?? "no branch"}</span>
            )
          ) : (
            <span className="text-sm text-zinc-500">{loading ? "Reading the repository…" : "No status yet"}</span>
          )}
          {status?.upstream ? (
            <span className="text-xs text-zinc-500">→ {status.upstream}</span>
          ) : (
            status && !status.detached && <span className="text-xs text-amber-400/90">not published yet</span>
          )}
          {status && status.ahead > 0 && (
            <span className="inline-flex items-center gap-0.5 text-xs text-emerald-400">
              <ArrowUp size={12} />
              {status.ahead}
            </span>
          )}
          {status && status.behind > 0 && (
            <span className="inline-flex items-center gap-0.5 text-xs text-sky-400">
              <ArrowDown size={12} />
              {status.behind}
            </span>
          )}
          <Button
            size="sm"
            variant="ghost"
            disabled={busy || !status || !!status.operation || !actions}
            title={opTitle ?? (busy ? "Wait for the running operation" : "Switch, create, rename or delete local branches")}
            onClick={() => actions?.branches()}
            className="px-2"
          >
            <GitBranch size={13} />
            Branches
          </Button>
          {status && <span className="text-xs text-zinc-600">{agoLabel(status.last_fetch_secs)}</span>}
        </div>

        <div className="flex items-center gap-2 flex-wrap">
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || !actions}
            onClick={() => actions?.fetch()}
            className="min-w-[5.5rem]"
            title={sub ? `git fetch inside ${sub} — updates its remote branches only, never its working tree` : "git fetch — updates remote branches only, never your working tree"}
          >
            <RefreshCw size={13} className={running && job?.kind === "fetch" ? "animate-spin" : ""} />
            Fetch
          </Button>
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || peeking}
            onClick={onPeek}
            className="min-w-[8.5rem]"
            title="Asks the remote where the branch is now — downloads nothing, writes nothing in this folder"
          >
            <Radar size={13} className={peeking ? "animate-pulse" : ""} />
            {peeking ? "Checking…" : "Check GitHub"}
          </Button>
          <Button
            size="sm"
            variant={status && status.behind > 0 ? "primary" : "secondary"}
            disabled={busy || !status?.upstream || pullBlocked !== null || !actions}
            title={pullBlocked ?? undefined}
            onClick={() => actions?.pull()}
            className="min-w-[8.5rem]"
          >
            <Download size={13} />
            Pull ({pullMode === "ff-only" ? "fast-forward" : pullMode})
          </Button>
          <Button
            size="sm"
            variant={status && status.behind > 0 ? "secondary" : "primary"}
            disabled={busy || !status || status.push.blocked || status.detached || status.unborn || !actions}
            title={status?.push.blocked ? status.push.reason : undefined}
            onClick={() => actions?.push()}
            className="min-w-[8.5rem]"
          >
            {status?.push.blocked ? <Ban size={13} /> : <Upload size={13} />}
            {status?.push.lock.locked
              ? "Push locked"
              : status?.push.blocked
                ? "Push blocked"
                : status?.upstream
                  ? `Push${status.ahead ? ` (${status.ahead})` : ""}`
                  : "Publish branch"}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            disabled={!actions || loading}
            onClick={() => actions?.refresh()}
            title="Re-read this repository"
            className="min-w-[6rem]"
          >
            {loading ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            {loading ? "Reading…" : "Refresh"}
          </Button>
        </div>
      </div>

      {g && (
        <p className="text-xs text-zinc-400 leading-relaxed" aria-label="Glance">
          <span className="text-zinc-500">{g.facts}</span>
          <span className="text-zinc-600"> · </span>
          <span className={status?.conflicted_count || status?.sync ? "text-amber-300" : "text-zinc-300"}>Next: {g.next}</span>
          {running && job && (
            <span className="inline-flex items-center gap-1 ml-2 text-sky-300">
              <Loader2 size={11} className="animate-spin" />
              {job.label}
            </span>
          )}
        </p>
      )}

      {peek && (
        <div className={`flex flex-wrap items-center gap-2 text-xs ${peek.changed ? "text-amber-200" : "text-zinc-400"}`}>
          <Radar size={13} className={`shrink-0 ${peek.changed ? "text-amber-400" : "text-emerald-400"}`} />
          <span className="break-words">{peek.message}</span>
          {peek.changed && !peek.branch_gone && actions && (
            <Button size="sm" variant="secondary" onClick={() => actions.fetch()} disabled={busy}>
              Fetch now
            </Button>
          )}
          <button type="button" onClick={onPeekDismiss} className="text-zinc-500 hover:text-zinc-300 cursor-pointer">
            dismiss
          </button>
        </div>
      )}
    </div>
  );
}
