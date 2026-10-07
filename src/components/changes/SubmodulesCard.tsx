import { useCallback, useEffect, useState, useRef } from "react";
import { Box, Download, Loader2, FolderOpen, GitCommitHorizontal, GitBranch, AlertTriangle, CornerDownRight } from "lucide-react";
import { Button } from "../Button";
import { ChangesList } from "./ChangesList";
import { DiffPanel } from "./DiffPanel";
import * as api from "../../lib/api";
import type { ChangeEntry, RepoStatus, SubmoduleInfo } from "../../lib/api";

interface Props {
  repoPath: string;
  busy: boolean;
  onOpenRepo: (path: string) => void;
  /** Initialise exactly these never-populated submodules. Populated ones are never touched. */
  onUpdate: (paths: string[]) => void;
  /** `git pull` inside one submodule, on its own branch. */
  onPullInside: (sub: SubmoduleInfo) => void;
  /** `git submodule update -- <path>`: check out the commit this repository records. */
  onUpdateOne: (path: string) => void;
  /** One commit in this repository recording where that submodule is checked out now. */
  onRecordOne: (path: string) => void;
  /** A branch at a detached submodule's checkout, named by the person. */
  onStartBranch: (path: string, name: string) => void;
  /** "<nonce>:<path>" — opens that submodule's branch form (a push that needs one). */
  openBranchFormFor?: string | null;
  /** Bumped by the page after any operation, so the list re-reads. */
  refreshKey: number;
  /** Submodules that have their own section in the main column. */
  sectionPaths?: string[];
  onJumpToSection?: (path: string) => void;
}

const quiet = (s: SubmoduleInfo) => !s.operation && s.conflicted === 0;
/** "Pull inside": a branch with an upstream, nothing in progress inside. */
export const canPullInside = (s: SubmoduleInfo) => s.initialised && quiet(s) && !!s.own_branch && !!s.own_upstream;
/**
 * "Update to <recorded>": the record moved past the checkout (a pull of this
 * repository does that). Never over commits of its own — those get recorded,
 * not checked out from under.
 */
export const canUpdate = (s: SubmoduleInfo) =>
  s.initialised && quiet(s) && s.behind > 0 && s.ahead === 0 && s.dirty_tracked === 0 && !s.recorded_missing;
export const pullInsideTitle = (s: SubmoduleInfo) =>
  `git pull inside ${s.path}: brings ${s.own_branch} up to date with ${s.own_upstream}${s.own_behind > 0 ? ` (${s.own_behind} behind)` : ""}. Nothing in this repository changes until you record the pointer.`;
export const updateTitle = (s: SubmoduleInfo) =>
  `git submodule update -- ${s.path}: checks out ${s.recorded_short}, the commit this repository records (${s.behind} commit${s.behind === 1 ? "" : "s"} past what is checked out now)`;
/**
 * "Record this pointer": the mirror case — the checkout moved past the record
 * (a commit or a pull inside did that), so the repository can record it. Only
 * when strictly ahead: equal, behind or diverged checkouts, and ones whose
 * recorded commit the submodule does not have yet, are the backend's refusals.
 */
export const canRecord = (s: SubmoduleInfo) =>
  s.initialised && s.listed && quiet(s) && s.ahead > 0 && s.behind === 0 && !s.recorded_missing;
export const recordTitle = (s: SubmoduleInfo) =>
  `One commit in this repository recording ${s.actual_short} for ${s.path}, ${s.ahead} commit${s.ahead === 1 ? "" : "s"} ahead of the ${s.recorded_short} it records now. Nothing inside ${s.path} changes.`;

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
    case "incomplete":
      return "bg-amber-500/15 text-amber-300";
    case "unmapped":
      return "bg-red-500/15 text-red-300";
  }
  return "bg-zinc-700/50 text-zinc-400";
}

function chipLabel(state: SubmoduleInfo["state"]): string {
  switch (state) {
    case "moved-and-dirty":
      return "moved + dirty";
    case "not-initialised":
      return "not downloaded";
    case "incomplete":
      return "download interrupted";
    case "unmapped":
      return "unfetchable";
    default:
      return state;
  }
}

/** Detached and populated: a branch at its checkout is what lets it keep and push its commits. */
export const canStartBranch = (s: SubmoduleInfo) => s.initialised && s.gitdir_valid && !s.own_branch && !!s.actual;

export function startBranchTitle(s: SubmoduleInfo): string {
  const n = s.own_unpushed ?? 0;
  return n > 0
    ? `Detached: ${n} commit${n === 1 ? "" : "s"} here ${n === 1 ? "is" : "are"} on no branch and no remote. A branch keeps them and lets you push them.`
    : "Detached: a branch here lets you commit and push";
}

/** The recorded commit is on none of the submodule's remotes: a clone of this repository could not check it out. */
export const recordedUnpushed = (s: SubmoduleInfo) => s.recorded_on_remote === false;

export function recordedUnpushedTitle(s: SubmoduleInfo): string {
  return `This repository records ${s.recorded_short} for ${s.path}, which none of ${s.path}'s remote-tracking branches has — as far as this checkout knows. A clone could not check it out: push ${s.path} first (or fetch it, if that commit was pushed from elsewhere).`;
}

/** Asks for the branch's name; Enter or Create submits it, Escape cancels. */
export function StartBranchForm({ path, busy, onSubmit, onCancel }: { path: string; busy: boolean; onSubmit: (name: string) => void; onCancel: () => void }) {
  const [name, setName] = useState("");
  const ready = name.trim().length > 0 && !busy;
  return (
    <form
      className="flex items-center gap-1.5 flex-wrap"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) onSubmit(name.trim());
      }}
    >
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel();
        }}
        placeholder="branch name"
        aria-label={`Branch name for ${path}`}
        className="h-7 w-40 rounded bg-zinc-900 border border-zinc-700 px-2 text-xs text-zinc-200 font-mono focus:outline-none focus:border-zinc-500"
      />
      <Button size="sm" variant="primary" type="submit" disabled={!ready} className="px-2">
        Create branch
      </Button>
      <Button size="sm" variant="ghost" type="button" onClick={onCancel} className="px-2">
        Cancel
      </Button>
    </form>
  );
}

/** One submodule, expandable to show what moved and what changed inside it. */
function SubmoduleRow({
  repoPath,
  sub,
  busy,
  onOpenRepo,
  onPullInside,
  onUpdateOne,
  onRecordOne,
  onStartBranch,
  openBranchFormFor,
  hasSection,
  onJumpToSection,
  refreshKey,
}: {
  repoPath: string;
  sub: SubmoduleInfo;
  busy: boolean;
  onOpenRepo: (path: string) => void;
  onPullInside: (sub: SubmoduleInfo) => void;
  onUpdateOne: (path: string) => void;
  onRecordOne: (path: string) => void;
  /** A branch at a detached submodule's checkout. */
  onStartBranch: (path: string, name: string) => void;
  /** "<nonce>:<path>" — the page asks this row to open its branch form. */
  openBranchFormFor?: string | null;
  /** The main column already shows this submodule's files — link there instead of re-reading them. */
  hasSection: boolean;
  onJumpToSection?: (path: string) => void;
  refreshKey: number;
}) {
  const [open, setOpen] = useState(false);
  const [inner, setInner] = useState<RepoStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [diffFor, setDiffFor] = useState<ChangeEntry | null>(null);
  const [branching, setBranching] = useState(false);
  // Asked by the page (a push refused for this detached submodule): open the
  // form; the input's focus brings the row into view.
  useEffect(() => {
    const path = openBranchFormFor?.split(":").slice(1).join(":");
    if (path === sub.path && canStartBranch(sub)) setBranching(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openBranchFormFor]);

  const absolute = `${repoPath.replace(/\/+$/, "")}/${sub.path}`;
  const hasInnerChanges = sub.dirty_tracked > 0 || sub.dirty_untracked > 0;
  const canToggle = sub.moved_commits.length > 0 || (sub.initialised && hasInnerChanges);

  // The file list is a snapshot of one state of the submodule. When the
  // backend's facts about it change, or anything ran, it is stale: drop it and
  // let the effect below re-read it if the row is still open.
  useEffect(() => {
    setInner(null);
    setError(null);
  }, [sub.actual, sub.dirty_tracked, sub.dirty_untracked, refreshKey]);

  // Read the submodule's own files only when asked: cheap per submodule
  // (~70 ms) but pointless for every one of them at once.
  // `loading` is deliberately NOT a dependency: setting it re-ran this effect,
  // whose cleanup cancelled the very request it had just started — and the
  // cancelled request never cleared the flag, so the row span forever.
  const inFlight = useRef(false);
  useEffect(() => {
    if (hasSection || !open || !sub.initialised || !hasInnerChanges || inner || inFlight.current) return;
    let cancelled = false;
    inFlight.current = true;
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
        inFlight.current = false;
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [hasSection, open, sub.initialised, hasInnerChanges, inner, absolute]);

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
            {recordedUnpushed(sub) && (
              <span className="text-[10px] px-1.5 py-0.5 rounded shrink-0 bg-amber-500/15 text-amber-300" title={recordedUnpushedTitle(sub)}>
                recorded commit not pushed
              </span>
            )}
            {sub.initialised && (
              <span
                className="text-[10px] font-mono px-1.5 py-0.5 rounded shrink-0 bg-zinc-900/60 text-zinc-400"
                title="This submodule's own branch, its upstream, and how far apart they are"
              >
                {sub.own_branch ?? "detached"}
                {sub.own_upstream ? ` → ${sub.own_upstream}` : ""}
                {sub.own_ahead > 0 ? ` ↑${sub.own_ahead}` : ""}
                {sub.own_behind > 0 ? ` ↓${sub.own_behind}` : ""}
              </span>
            )}
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

          {canToggle && (
            <button
              onClick={() => setOpen((o) => !o)}
              className="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer mt-1"
            >
              {open ? "Hide details" : "Show what changed"}
            </button>
          )}

          {/* Only while the toggle exists: once the submodule is clean there is nothing to keep showing. */}
          {open && canToggle && (
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

        <div className="shrink-0 flex flex-wrap justify-end items-center gap-0.5 max-w-[50%]">
          {canPullInside(sub) && (
            <Button
              size="sm"
              variant={sub.own_behind > 0 ? "secondary" : "ghost"}
              disabled={busy}
              onClick={() => onPullInside(sub)}
              title={pullInsideTitle(sub)}
              className="px-1.5"
            >
              <Download size={13} />
              Pull inside{sub.own_behind > 0 ? ` (${sub.own_behind})` : ""}
            </Button>
          )}
          {canUpdate(sub) && (
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => onUpdateOne(sub.path)} title={updateTitle(sub)} className="px-1.5">
              <GitCommitHorizontal size={13} />
              Update to {sub.recorded_short}
            </Button>
          )}
          {canRecord(sub) && (
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => onRecordOne(sub.path)} title={recordTitle(sub)} className="px-1.5">
              <GitCommitHorizontal size={13} />
              Record this pointer
            </Button>
          )}
          {canStartBranch(sub) && !branching && (
            <Button size="sm" variant={(sub.own_unpushed ?? 0) > 0 ? "secondary" : "ghost"} disabled={busy} onClick={() => setBranching(true)} title={startBranchTitle(sub)} className="px-1.5">
              <GitBranch size={13} />
              Start a branch here
            </Button>
          )}
          {canStartBranch(sub) && branching && (
            <StartBranchForm
              path={sub.path}
              busy={busy}
              onSubmit={(name) => {
                setBranching(false);
                onStartBranch(sub.path, name);
              }}
              onCancel={() => setBranching(false)}
            />
          )}
          {sub.initialised && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => onOpenRepo(absolute)}
              title="Work in this submodule as its own repository"
              className="px-1.5"
            >
              <FolderOpen size={13} />
              Open
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
  onPullInside,
  onUpdateOne,
  onRecordOne,
  onStartBranch,
  openBranchFormFor,
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

  // An interrupted download is not populated: it is downloadable again.
  const present = subs.filter((s) => s.initialised && s.state !== "incomplete");
  const missing = subs.filter((s) => !s.initialised || s.state === "incomplete");
  const fetchable = missing.filter((s) => s.listed).length;
  const unfetchable = missing.length - fetchable;
  const interrupted = missing.filter((s) => s.state === "incomplete").length;

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40">
      <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-zinc-700/50">
        <div className="flex items-center gap-2 min-w-0">
          <Box size={14} className="text-zinc-400 shrink-0" />
          <h3 className="text-sm font-medium text-zinc-200 truncate min-w-0">Submodules</h3>
          <span className="text-xs text-zinc-500 shrink-0">{subs.length}</span>
          {loading && <Loader2 size={12} className="animate-spin text-zinc-500 shrink-0" />}
        </div>
        {fetchable > 0 && (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => onUpdate(missing.filter((s) => s.listed).map((s) => s.path))}
            title="git submodule update --init for the never-initialised ones only — populated submodules are left exactly as they are"
          >
            <Download size={13} />
            Download missing ({fetchable})
          </Button>
        )}
      </div>

      {error && <p className="px-3 py-2 text-xs text-red-400 break-words">{error}</p>}

      <ul className="max-h-96 overflow-y-auto divide-y divide-zinc-800/70">
        {present.map((s) => (
          <SubmoduleRow
            key={`${repoPath}/${s.path}`}
            repoPath={repoPath}
            sub={s}
            busy={busy}
            onOpenRepo={onOpenRepo}
            onPullInside={onPullInside}
            onUpdateOne={onUpdateOne}
            onRecordOne={onRecordOne}
            onStartBranch={onStartBranch}
            openBranchFormFor={openBranchFormFor}
            hasSection={sectionPaths.includes(s.path)}
            onJumpToSection={onJumpToSection}
            refreshKey={refreshKey}
          />
        ))}
      </ul>

      {missing.length > 0 && (
        <div className="px-3 py-2 border-t border-zinc-700/50">
          <div className="flex items-start gap-2">
            <AlertTriangle size={13} className="shrink-0 mt-0.5 text-zinc-500" />
            <div className="min-w-0 flex-1">
              <p className="text-xs text-zinc-400 leading-relaxed">
                {missing.length} not downloaded yet
                {interrupted > 0 && ` (${interrupted} download${interrupted === 1 ? "" : "s"} interrupted — Download finishes ${interrupted === 1 ? "it" : "them"})`}
                {fetchable > 0 && unfetchable > 0
                  ? ` — ${fetchable} can be fetched with Download missing, ${unfetchable} have no entry in .gitmodules so git can't fetch them`
                  : fetchable > 0
                    ? " — Download missing fetches them (git submodule update --init)"
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
                      key={`${repoPath}/${s.path}`}
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
