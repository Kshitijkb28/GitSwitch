import { useCallback, useEffect, useState } from "react";
import { Box, Download, Loader2, FolderOpen, GitCommitHorizontal, AlertTriangle, CornerDownRight } from "lucide-react";
import { Button } from "../Button";
import { ChangesList } from "./ChangesList";
import { DiffPanel } from "./DiffPanel";
import * as api from "../../lib/api";
import type { ChangeEntry, RepoStatus, SubmoduleInfo } from "../../lib/api";

interface Props {
  repoPath: string;
  busy: boolean;
  onOpenRepo: (path: string) => void;
  onUpdate: () => void;
  /** Bumped by the page after any operation, so the list re-reads. */
  refreshKey: number;
  /** Submodules that have their own section in the main column. */
  sectionPaths?: string[];
  onJumpToSection?: (path: string) => void;
}

/** A submodule is a repo, so its files open in the same diff panel. */
function chipClass(state: SubmoduleInfo["state"]): string {
  switch (state) {
    case "clean":
      return "bg-zinc-700/50 text-zinc-400";
    case "moved":
      return "bg-sky-500/15 text-sky-300";
    case "dirty":
      return "bg-amber-500/15 text-amber-300";
    case "moved-and-dirty":
      return "bg-amber-500/15 text-amber-300";
    case "not-initialised":
      return "bg-zinc-700/50 text-zinc-400";
    case "unmapped":
      return "bg-red-500/15 text-red-300";
  }
}

function chipLabel(state: SubmoduleInfo["state"]): string {
  switch (state) {
    case "moved-and-dirty":
      return "moved + dirty";
    case "not-initialised":
      return "not initialised";
    case "unmapped":
      return "unfetchable";
    default:
      return state;
  }
}

/** One submodule, expandable to show what moved and what changed inside it. */
function SubmoduleRow({
  repoPath,
  sub,
  busy,
  onOpenRepo,
  hasSection,
  onJumpToSection,
}: {
  repoPath: string;
  sub: SubmoduleInfo;
  busy: boolean;
  onOpenRepo: (path: string) => void;
  /** The main column already shows this submodule's files — link there instead of re-reading them. */
  hasSection: boolean;
  onJumpToSection?: (path: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [inner, setInner] = useState<RepoStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [diffFor, setDiffFor] = useState<ChangeEntry | null>(null);

  const absolute = `${repoPath.replace(/\/+$/, "")}/${sub.path}`;
  const hasInnerChanges = sub.dirty_tracked > 0 || sub.dirty_untracked > 0;

  // Read the submodule's own files only when asked: cheap per submodule
  // (~70 ms) but pointless for every one of them at once.
  useEffect(() => {
    if (hasSection || !open || !sub.initialised || !hasInnerChanges || inner || loading) return;
    let cancelled = false;
    setLoading(true);
    api
      .changesRepoStatus(absolute)
      .then((s) => {
        if (!cancelled) setInner(s);
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
  }, [hasSection, open, sub.initialised, hasInnerChanges, inner, loading, absolute]);

  const innerEntries = inner?.entries ?? [];

  return (
    <li className="px-3 py-2">
      <div className="flex items-start gap-2">
        <Box size={13} className="shrink-0 mt-1 text-zinc-500" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-xs font-mono text-zinc-200 truncate min-w-0">{sub.path}</span>
            <span
              className={`text-[10px] px-1.5 py-0.5 rounded shrink-0 ${chipClass(sub.state)}`}
            >
              {chipLabel(sub.state)}
            </span>
          </div>
          <p className="text-xs text-zinc-400 leading-relaxed mt-0.5">{sub.summary}</p>

          {sub.initialised && (
            <p className="text-[10px] font-mono text-zinc-600 mt-0.5">
              records {sub.recorded_short}
              {sub.actual_short && sub.actual_short !== sub.recorded_short
                ? ` · checked out ${sub.actual_short}`
                : ""}
            </p>
          )}

          {(sub.moved_commits.length > 0 || (sub.initialised && hasInnerChanges)) && (
            <button
              onClick={() => setOpen((o) => !o)}
              className="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer mt-1"
            >
              {open ? "Hide details" : "Show what changed"}
            </button>
          )}

          {open && (
            <div className="mt-2 space-y-2">
              {sub.moved_commits.length > 0 && (
                <div className="rounded-lg border border-zinc-700/50 bg-zinc-900/40 p-2">
                  <p className="text-[10px] uppercase tracking-wide text-zinc-500 mb-1">
                    Commits it moved through
                  </p>
                  <ul className="space-y-0.5">
                    {sub.moved_commits.map((c) => (
                      <li key={c.short} className="flex items-start gap-2 text-xs min-w-0">
                        <GitCommitHorizontal size={11} className="shrink-0 mt-0.5 text-zinc-600" />
                        <span className="font-mono text-zinc-500 shrink-0">{c.short}</span>
                        <span className="text-zinc-400 truncate min-w-0">{c.subject}</span>
                      </li>
                    ))}
                    {sub.more_moved > 0 && (
                      <li className="text-xs text-zinc-600">…and {sub.more_moved} more</li>
                    )}
                  </ul>
                </div>
              )}

              {hasSection && hasInnerChanges && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="text-sky-300"
                  onClick={() => onJumpToSection?.(sub.path)}
                  title="Its changed files are listed in their own section on this page"
                >
                  <CornerDownRight size={12} />
                  See the changes inside ↓
                </Button>
              )}
              {loading && (
                <p className="flex items-center gap-1.5 text-xs text-zinc-500">
                  <Loader2 size={12} className="animate-spin" />
                  Reading what changed inside…
                </p>
              )}
              {error && <p className="text-xs text-red-400 break-words">{error}</p>}
              {!hasSection && innerEntries.length > 0 && (
                <ChangesList
                  title={`Changed inside ${sub.path}`}
                  icon={<Box size={13} />}
                  entries={innerEntries}
                  side="unstaged"
                  busy={busy}
                  // Only onOpen: passing no action callbacks is how ChangesList
                  // renders read-only. Staging here would commit into a
                  // different repository than the one this page is showing.
                  onOpen={setDiffFor}
                />
              )}
              {inner?.untracked_truncated && (
                <p className="text-xs text-amber-400/90">
                  Only the first 2000 untracked files are listed.
                </p>
              )}
            </div>
          )}
        </div>

        <div className="shrink-0">
          {sub.initialised && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => onOpenRepo(absolute)}
              title="Work in this submodule as its own repository"
              className="px-1.5"
            >
              <FolderOpen size={13} />
            </Button>
          )}
        </div>
      </div>

      {diffFor && (
        <DiffPanel repoPath={absolute} entry={diffFor} onClose={() => setDiffFor(null)} />
      )}
    </li>
  );
}

export function SubmodulesCard({
  repoPath,
  busy,
  onOpenRepo,
  onUpdate,
  refreshKey,
  sectionPaths = [],
  onJumpToSection,
}: Props) {
  const [subs, setSubs] = useState<SubmoduleInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showAllMissing, setShowAllMissing] = useState(false);

  const load = useCallback(() => {
    if (!repoPath) return;
    setLoading(true);
    api
      .changesSubmodules(repoPath)
      .then((s) => {
        setSubs(s);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [repoPath]);

  useEffect(load, [load, refreshKey]);

  if (!loading && subs.length === 0 && !error) return null;

  const present = subs.filter((s) => s.initialised);
  const missing = subs.filter((s) => !s.initialised);
  const fetchable = missing.filter((s) => s.listed).length;
  const unfetchable = missing.length - fetchable;

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40">
      <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-zinc-700/50">
        <div className="flex items-center gap-2 min-w-0">
          <Box size={14} className="text-zinc-400 shrink-0" />
          <h3 className="text-sm font-medium text-zinc-200 truncate min-w-0">Submodules</h3>
          <span className="text-xs text-zinc-500 shrink-0">{subs.length}</span>
          {loading && <Loader2 size={12} className="animate-spin text-zinc-500 shrink-0" />}
        </div>
        <Button size="sm" variant="ghost" disabled={busy} onClick={onUpdate}>
          <Download size={13} />
          Update
        </Button>
      </div>

      {error && <p className="px-3 py-2 text-xs text-red-400 break-words">{error}</p>}

      <ul className="max-h-96 overflow-y-auto divide-y divide-zinc-800/70">
        {present.map((s) => (
          <SubmoduleRow
            key={s.path}
            repoPath={repoPath}
            sub={s}
            busy={busy}
            onOpenRepo={onOpenRepo}
            hasSection={sectionPaths.includes(s.path)}
            onJumpToSection={onJumpToSection}
          />
        ))}
      </ul>

      {missing.length > 0 && (
        <div className="px-3 py-2 border-t border-zinc-700/50">
          <div className="flex items-start gap-2">
            <AlertTriangle size={13} className="shrink-0 mt-0.5 text-zinc-500" />
            <div className="min-w-0 flex-1">
              <p className="text-xs text-zinc-400 leading-relaxed">
                {missing.length} not initialised
                {fetchable > 0 && unfetchable > 0
                  ? ` — ${fetchable} can be fetched with Update, ${unfetchable} have no entry in .gitmodules so git can't fetch them`
                  : fetchable > 0
                    ? " — Update submodules will fetch them"
                    : " — none of them have an entry in .gitmodules, so git can't fetch them"}
                .
              </p>
              <button
                onClick={() => setShowAllMissing((v) => !v)}
                className="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer mt-1"
              >
                {showAllMissing ? "Hide them" : "List them"}
              </button>
              {showAllMissing && (
                <ul className="mt-1 max-h-40 overflow-y-auto space-y-0.5">
                  {missing.map((s) => (
                    <li
                      key={s.path}
                      className="text-xs font-mono text-zinc-500 truncate"
                      title={s.summary}
                    >
                      {s.path}
                      {!s.listed && <span className="text-red-400/70"> · unfetchable</span>}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
