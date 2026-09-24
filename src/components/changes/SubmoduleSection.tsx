import { useState } from "react";
import { AlertTriangle, Box, ChevronDown, ChevronRight, CircleSlash, FolderOpen, Play } from "lucide-react";
import { Badge } from "../Badge";
import { Button } from "../Button";
import { CommitBox } from "./CommitBox";
import { DiffPanel } from "./DiffPanel";
import { TreeLists, type DiscardVariant } from "./TreeLists";
import * as api from "../../lib/api";
import type { ChangeEntry, OpResult, RepoStatus, SubmoduleStatus } from "../../lib/api";

/** The element id a jump link scrolls to. */
export function sectionId(path: string): string {
  return `submodule-section-${path.replace(/\//g, "->")}`;
}

/** A submodule earns its own section only when there is something to do in it. */
export function needsSection(s: RepoStatus): boolean {
  return s.entries.length > 0 || s.ahead > 0 || !!s.operation || s.detached || s.conflicted_count > 0;
}

type Apply = (key: string, fn: () => Promise<OpResult>, target?: string) => Promise<OpResult | undefined>;

type Props = {
  sub: SubmoduleStatus;
  parentRepoPath: string;
  parentStatus: RepoStatus;
  open: boolean;
  onToggle: () => void;
  busy: boolean;
  draft: string;
  onDraft: (m: string) => void;
  /** The page's `apply`; `target` = this submodule's path routes the status back here. */
  apply: Apply;
  onOpenRepo: (path: string) => void;
  /** Asks first — the page owns the confirmation, keyed to this submodule. */
  onDiscard: (paths: string[], variant: DiscardVariant) => void;
  onSwitchBranch?: () => void;
};

/**
 * One submodule's working tree, inline in the parent's page. Every action
 * here runs inside the submodule; the one thing that belongs to the parent —
 * recording the new commit as the pointer — is offered right after a commit,
 * because that is the step people forget.
 */
export function SubmoduleSection({
  sub,
  parentRepoPath,
  parentStatus,
  open,
  onToggle,
  busy,
  draft,
  onDraft,
  apply,
  onOpenRepo,
  onDiscard,
  onSwitchBranch,
}: Props) {
  const st = sub.status;
  const path = st.path;
  const [amend, setAmend] = useState(false);
  const [diffFor, setDiffFor] = useState<ChangeEntry | null>(null);
  // The parent status at commit time: until it is re-read, the footer stays;
  // afterwards it stays only while the gitlink still points at the old commit.
  const [committed, setCommitted] = useState<{ short: string; parent: RepoStatus } | null>(null);

  const parentRow = parentStatus.entries.find((e) => e.is_submodule && e.path === sub.path);
  const showFooter =
    committed !== null && (parentStatus === committed.parent || parentRow?.sub_commit_changed === true);
  const changed = st.entries.filter((e) => e.kind === "tracked").length;
  const message = draft || (st.merge_message ?? "");

  const onCommit = async () => {
    const parentAtCommit = parentStatus;
    const r = await apply("commit", () => api.changesCommit(path, message, amend), path);
    if (r?.ok) {
      onDraft("");
      setAmend(false);
      setCommitted({
        short: r.commit?.short ?? (r.status?.head_oid ?? "").slice(0, 7),
        parent: parentAtCommit,
      });
    }
  };

  const op = st.operation;

  return (
    <section
      id={sectionId(sub.path)}
      className="rounded-xl border border-sky-500/20 bg-zinc-800/30 scroll-mt-4"
    >
      <div className="flex flex-wrap items-center gap-2 px-3 py-2">
        <button
          aria-expanded={open}
          aria-label={`Toggle submodule ${sub.path}`}
          onClick={onToggle}
          className="flex flex-wrap items-center gap-x-2 gap-y-1 min-w-[16rem] flex-1 text-left cursor-pointer group"
        >
          {open ? (
            <ChevronDown size={14} className="shrink-0 text-zinc-500" />
          ) : (
            <ChevronRight size={14} className="shrink-0 text-zinc-500" />
          )}
          <Box size={13} className="shrink-0 text-sky-300/80" />
          <span className="text-sm text-zinc-200 truncate min-w-0 flex-1 basis-40 group-hover:text-zinc-100">
            Inside <span className="font-mono">{sub.path}</span>
            {!st.detached && st.branch && (
              <>
                {" · on "}
                <span className="font-mono">{st.branch}</span>
              </>
            )}
            {st.ahead > 0 && (
              <>
                {" · "}
                <span className="text-emerald-400">↑{st.ahead}</span>
              </>
            )}
            {changed > 0 && ` · ${changed} changed`}
            {st.untracked_count > 0 && ` · ${st.untracked_count} new`}
          </span>
          <span className="flex items-center gap-1 shrink-0">
            {st.detached && <Badge variant="warning">detached</Badge>}
            {op && <Badge variant="warning">{op.label.toLowerCase()}</Badge>}
            {st.conflicted_count > 0 && (
              <Badge variant="error">
                {st.conflicted_count} conflict{st.conflicted_count === 1 ? "" : "s"}
              </Badge>
            )}
          </span>
        </button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onOpenRepo(path)}
          title="Work in this submodule as its own repository"
        >
          <FolderOpen size={13} />
          Open as its own repository
        </Button>
      </div>

      {open && (
        <div className="border-t border-zinc-700/50 p-3 space-y-3">
          {op && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2">
              <AlertTriangle size={14} className="text-amber-400 shrink-0" />
              <p className="text-xs text-amber-100 flex-1 min-w-0 break-words">
                {op.label} inside <span className="font-mono">{sub.path}</span>
                <span className="text-amber-200/70"> — {op.detail || "Finish it, or abort to go back."}</span>
              </p>
              {op.continue_command && op.kind !== "merge" && (
                <Button
                  size="sm"
                  variant="primary"
                  disabled={busy || st.conflicted_count > 0}
                  title={st.conflicted_count > 0 ? "Resolve and stage every conflicted file first" : op.continue_command}
                  onClick={() => apply("continue", () => api.changesContinue(path), path)}
                >
                  <Play size={13} />
                  Continue in {sub.path}
                </Button>
              )}
              <Button
                size="sm"
                variant="secondary"
                disabled={busy}
                title={op.abort_command}
                onClick={() => apply("abort", () => api.changesAbort(path), path)}
              >
                <CircleSlash size={13} />
                Abort in {sub.path}
              </Button>
            </div>
          )}

          <TreeLists
            status={st}
            busy={busy}
            onOpen={setDiffFor}
            onStage={(paths) => apply("stage", () => api.changesStage(path, paths), path)}
            onUnstage={(paths) => apply("unstage", () => api.changesUnstage(path, paths), path)}
            onDiscard={onDiscard}
            onResolveSide={(paths, side) => apply("resolve", () => api.changesResolveSide(path, paths, side), path)}
          />

          <CommitBox
            status={st}
            message={message}
            onMessage={onDraft}
            amend={amend}
            onAmend={setAmend}
            busy={busy}
            onCommit={onCommit}
            onSwitchBranch={onSwitchBranch}
            scopeLabel={sub.path}
          />

          {showFooter && committed && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border border-sky-500/30 bg-sky-500/5 px-3 py-2">
              <p className="text-xs text-sky-100 flex-1 min-w-0 break-words">
                Committed <span className="font-mono">{committed.short}</span> inside{" "}
                <span className="font-mono">{sub.path}</span>. This repository still records the old commit
                for <span className="font-mono">{sub.path}</span>.
              </p>
              <Button
                size="sm"
                variant="secondary"
                disabled={busy}
                title="Stages the new commit as this repository's pointer; commit it above"
                onClick={() => apply("stage", () => api.changesStage(parentRepoPath, [sub.path]))}
              >
                Stage pointer
              </Button>
              <Button
                size="sm"
                variant="secondary"
                disabled={busy}
                title="One commit in this repository recording every submodule's current commit"
                onClick={() => apply("record", () => api.changesRecordPointers(parentRepoPath))}
              >
                Record submodule pointers
              </Button>
            </div>
          )}
        </div>
      )}

      {diffFor && <DiffPanel repoPath={path} entry={diffFor} onClose={() => setDiffFor(null)} />}
    </section>
  );
}
