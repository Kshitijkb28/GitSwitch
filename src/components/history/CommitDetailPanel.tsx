import { useEffect, useState, type FormEvent } from "react";
import {
  X,
  FileText,
  ShieldCheck,
  ChevronRight,
  Copy,
  Undo2,
  GitBranchPlus,
  RotateCcw,
  Cherry,
  Loader2,
  AlertTriangle,
} from "lucide-react";
import { Badge } from "../Badge";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { useToast } from "../Toast";
import { DiffView } from "../changes/DiffPanel";
import { useGitJob } from "../../lib/gitJobs";
import * as api from "../../lib/api";
import type {
  CommitDetail,
  CommitTarget,
  FileDiff,
  OpResult,
  RepoStatus,
  ResetMode,
  TreeCommitRef,
} from "../../lib/api";

type Props = {
  detail: CommitDetail;
  /** Known when the commit was reached through "Go to commit"; null from a row click. */
  target: CommitTarget | null;
  repoPath: string;
  currentBranch: string | null;
  detached: boolean;
  busy: boolean;
  onClose: () => void;
  /** Runs one git operation; the page owns busy state, toasts, the result card and reloads. */
  onAction: (key: string, fn: () => Promise<OpResult>) => Promise<OpResult | undefined>;
  onResolve: (hash: string) => Promise<CommitTarget | null>;
};

type Mode = null | "goback" | "branch";

/**
 * What a hard reset would throw away — and what "stash first" sets aside.
 * New files are touched by neither, so they don't count.
 */
function isDirty(s: RepoStatus | null): boolean {
  if (!s) return false;
  return s.staged_count + s.unstaged_count > 0;
}

/** Clipboard write with a fallback for webviews that hide `navigator.clipboard`. */
async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the textarea trick
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    ta.style.pointerEvents = "none";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

/** A dark-theme radio, drawn the same way Checkbox is (the native one paints white). */
function Radio({
  checked,
  onChange,
  name,
  children,
}: {
  checked: boolean;
  onChange: () => void;
  name: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex items-start gap-2.5 cursor-pointer select-none min-w-0">
      <span className="relative inline-flex shrink-0 mt-0.5">
        <input
          type="radio"
          name={name}
          checked={checked}
          onChange={onChange}
          className="peer absolute inset-0 w-full h-full opacity-0 m-0 cursor-pointer"
        />
        <span
          aria-hidden="true"
          className={`w-[16px] h-[16px] rounded-full border flex items-center justify-center transition-colors
            peer-focus-visible:ring-2 peer-focus-visible:ring-emerald-500/50 peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-zinc-900
            ${checked ? "border-emerald-500" : "border-zinc-600 bg-zinc-800 peer-hover:border-zinc-500"}`}
        >
          {checked && <span className="w-2 h-2 rounded-full bg-emerald-500" />}
        </span>
      </span>
      <span className="min-w-0 text-sm text-zinc-200 break-words">{children}</span>
    </label>
  );
}

/**
 * Everything about one commit, plus the few things you can do from it. Each
 * action either opens an inline block (the "go back" chooser, the branch name
 * form) or a confirm modal (hard reset, revert) — never one overlay on top of
 * another inline block.
 */
export function CommitDetailPanel({
  detail,
  target,
  repoPath,
  currentBranch,
  detached,
  busy,
  onClose,
  onAction,
  onResolve,
}: Props) {
  const toast = useToast();
  const job = useGitJob(repoPath);
  const jobRunning = !!job?.running;
  const locked = busy || jobRunning;
  const lockTitle = jobRunning ? "Wait for the running operation" : undefined;
  const branch = currentBranch ?? "HEAD";
  const isMergeCommit = detail.parents.length > 1;

  const [mode, setMode] = useState<Mode>(null);

  // --- go back ---------------------------------------------------------------
  const [resolved, setResolved] = useState<CommitTarget | null>(target);
  const [resolving, setResolving] = useState(false);
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [stash, setStash] = useState(false);
  const [hardConfirm, setHardConfirm] = useState(false);
  /** The backend said a reset would drop pushed commits — same note as `would_drop_pushed`. */
  const [pushedNote, setPushedNote] = useState(false);

  // --- start a branch --------------------------------------------------------
  const [branchName, setBranchName] = useState("");

  // --- revert ----------------------------------------------------------------
  const [revertOpen, setRevertOpen] = useState(false);
  const [mainline, setMainline] = useState<1 | 2>(1);
  /** Parents the backend asked us to choose between (`merge-commit-needs-mainline`). */
  const [revertParents, setRevertParents] = useState<TreeCommitRef[] | null>(null);

  // --- file diffs ------------------------------------------------------------
  const [openFile, setOpenFile] = useState<string | null>(null);
  const [fileDiff, setFileDiff] = useState<FileDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);

  useEffect(() => {
    if (!openFile) return;
    let cancelled = false;
    setDiffLoading(true);
    setDiffError(null);
    setFileDiff(null);
    api
      .historyCommitFileDiff(repoPath, detail.hash, openFile)
      .then((d) => { if (!cancelled) setFileDiff(d); })
      .catch((e) => { if (!cancelled) setDiffError(String(e)); })
      .finally(() => { if (!cancelled) setDiffLoading(false); });
    return () => { cancelled = true; };
  }, [repoPath, detail.hash, openFile]);

  const dirty = isDirty(status);
  const pushed = pushedNote || resolved?.would_drop_pushed === true;
  const notBehind = resolved !== null && !resolved.contained_in_head;
  const atHead = resolved?.is_head === true;

  /** Why the "move the branch" rows can't be used right now, if they can't. */
  const moveBlock: string | null = locked
    ? lockTitle ?? "Working…"
    : resolving
      ? "Checking where this commit sits…"
      : notBehind
        ? `This commit is not behind ${branch}`
        : pushed
          ? "Those commits are already on the upstream"
          : atHead
            ? `${branch} is already here`
            : null;

  function openGoBack() {
    setMode("goback");
    setHardConfirm(false);
    setPushedNote(false);
    setResolving(true);
    onResolve(detail.hash)
      .then((t) => setResolved(t))
      .catch(() => setResolved(null))
      .finally(() => setResolving(false));
    api
      .changesRepoStatus(repoPath)
      .then((s) => { setStatus(s); setStash(isDirty(s)); })
      .catch(() => { setStatus(null); setStash(false); });
  }

  function openBranch() {
    setMode("branch");
    setHardConfirm(false);
  }

  function openRevert() {
    setRevertOpen(true);
    setMainline(1);
    // A merge's parents only carry subjects through resolve; fetch them so the
    // radios can say what each side is, but don't hold the modal for it.
    if (isMergeCommit && !resolved) {
      onResolve(detail.hash).then((t) => t && setResolved(t)).catch(() => {});
    }
  }

  /** Handles the two refusals that are answered inside the panel rather than by closing it. */
  function afterAction(r: OpResult | undefined) {
    if (!r || r.ok || !r.refusal) return;
    if (r.refusal.code === "would-drop-pushed") {
      setHardConfirm(false);
      setMode("goback");
      setPushedNote(true);
    } else if (r.refusal.code === "merge-commit-needs-mainline" && r.tree?.parents?.length) {
      setRevertParents(r.tree.parents);
      setMainline(1);
      setRevertOpen(true);
    }
  }

  async function detach() {
    afterAction(await onAction("detach", () => api.changesDetach(repoPath, detail.hash)));
  }

  async function reset(m: ResetMode) {
    setHardConfirm(false);
    // Only a hard reset touches the working tree, so only it offers (and
    // sends) the safety stash; soft and mixed leave your edits where they are.
    afterAction(await onAction("reset", () => api.changesReset(repoPath, detail.hash, m, m === "hard" && stash)));
  }

  async function createBranch(e: FormEvent) {
    e.preventDefault();
    const name = branchName.trim();
    if (!name) return;
    afterAction(await onAction("branch", () => api.changesCreateBranch(repoPath, name, detail.hash, true)));
  }

  const showMainline = revertParents !== null || isMergeCommit;
  const mainlineParents: TreeCommitRef[] =
    revertParents ??
    (resolved && resolved.oid === detail.hash && resolved.parents.length > 1
      ? resolved.parents
      : detail.parents.map((p) => ({ oid: p, short: p.slice(0, 7), subject: "" })));

  async function revert() {
    setRevertOpen(false);
    afterAction(
      await onAction("revert", () =>
        api.changesRevert(repoPath, detail.hash, showMainline ? mainline : null)
      )
    );
  }

  async function cherryPick() {
    afterAction(await onAction("cherry-pick", () => api.changesCherryPick(repoPath, detail.hash)));
  }

  async function copyHash() {
    if (await copyText(detail.hash)) toast.success(`Copied ${detail.short}…`);
    else toast.error("Could not copy the hash");
  }

  // Revert and cherry-pick both make a commit, which the backend refuses on a
  // detached HEAD — so both are gated here rather than after a confirm dialog.
  const needsBranch = locked || detached || !currentBranch;
  const needsBranchTitle = lockTitle ?? (detached || !currentBranch ? "Switch to a branch first" : undefined);

  const dropped = resolved && resolved.oid === detail.hash ? resolved.dropped_if_reset : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />
      <div className="relative w-full max-w-3xl max-h-[85vh] overflow-y-auto rounded-xl border border-zinc-700/50 bg-zinc-900 p-6 shadow-2xl min-w-0">
        <div className="flex items-start justify-between gap-3 mb-4">
          <div className="min-w-0">
            <h2 className="text-lg font-semibold text-zinc-100 break-words">
              {detail.subject}
            </h2>
            <p className="text-xs text-zinc-500 font-mono mt-1 break-all">{detail.hash}</p>
          </div>
          <button
            onClick={onClose}
            aria-label="Close"
            className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
          >
            <X size={18} />
          </button>
        </div>

        <div className="flex flex-wrap gap-2 mb-3">
          {isMergeCommit && <Badge variant="success">merge commit</Badge>}
          {detail.refs.map((r) => (
            <Badge key={r} variant="default">{r}</Badge>
          ))}
          {detail.signature && (
            <span className="inline-flex items-center gap-1 text-xs text-emerald-400">
              <ShieldCheck size={13} />
              {detail.signature}
            </span>
          )}
        </div>

        {/* ---------------- actions ---------------- */}
        <div className="flex flex-wrap gap-1.5 mb-4" aria-label="Commit actions">
          <Button size="sm" variant="ghost" disabled={locked} title={lockTitle} onClick={openGoBack}>
            <Undo2 size={13} />
            Go back to this commit…
          </Button>
          <Button size="sm" variant="ghost" disabled={locked} title={lockTitle} onClick={openBranch}>
            <GitBranchPlus size={13} />
            Start a branch here
          </Button>
          <Button size="sm" variant="ghost" disabled={needsBranch} title={needsBranchTitle} onClick={openRevert}>
            <RotateCcw size={13} />
            Undo this commit (revert)
          </Button>
          <Button size="sm" variant="ghost" disabled={needsBranch} title={needsBranchTitle} onClick={cherryPick}>
            <Cherry size={13} />
            Cherry-pick onto {currentBranch ?? "a branch"}
          </Button>
          <Button size="sm" variant="ghost" onClick={copyHash}>
            <Copy size={13} />
            Copy hash
          </Button>
          {(busy || jobRunning) && (
            <span className="inline-flex items-center gap-1.5 text-xs text-zinc-500 px-2">
              <Loader2 size={12} className="animate-spin text-emerald-400" />
              {jobRunning && !busy ? job?.label ?? "Working…" : "Working…"}
            </span>
          )}
        </div>

        {/* ---------------- go back chooser ---------------- */}
        {mode === "goback" && (
          <div className="rounded-lg border border-zinc-700/50 bg-zinc-800/40 p-3 mb-4 min-w-0">
            <div className="flex items-start justify-between gap-2 mb-2">
              <div className="min-w-0">
                <h3 className="text-sm font-medium text-zinc-200">
                  Go back to <span className="font-mono">{detail.short}</span>
                </h3>
                <p className="text-xs text-zinc-500 mt-0.5">
                  Pick how far the change reaches. Nothing here touches the remote.
                </p>
              </div>
              <button
                onClick={() => setMode(null)}
                aria-label="Close chooser"
                className="p-1 rounded-lg hover:bg-zinc-700 text-zinc-500 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
              >
                <X size={14} />
              </button>
            </div>

            {resolving && (
              <p className="inline-flex items-center gap-1.5 text-xs text-zinc-500 mb-2">
                <Loader2 size={12} className="animate-spin text-emerald-400" />
                Checking where this commit sits…
              </p>
            )}

            {pushed && (
              <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2.5 mb-3 min-w-0">
                <p className="flex items-start gap-2 text-xs text-amber-300 break-words">
                  <AlertTriangle size={14} className="shrink-0 mt-0.5" />
                  <span>
                    Moving {branch} back would drop commits already on the upstream, which needs a
                    force-push — GitSwitch never does that.
                  </span>
                </p>
                <div className="flex flex-wrap gap-1.5 mt-2">
                  <Button size="sm" variant="secondary" disabled={needsBranch} title={needsBranchTitle} onClick={openRevert}>
                    <RotateCcw size={13} />
                    Undo this commit (revert)
                  </Button>
                  <Button size="sm" variant="secondary" disabled={locked} title={lockTitle} onClick={openBranch}>
                    <GitBranchPlus size={13} />
                    Start a branch here
                  </Button>
                </div>
              </div>
            )}

            <ul role="radiogroup" aria-label="Go back mode" className="space-y-1.5">
              <GoBackRow
                title="Look at it"
                consequence="Checks this commit out without moving any branch (detached HEAD). Nothing is lost."
                disabled={locked}
                disabledTitle={lockTitle}
                onDo={detach}
              />
              <GoBackRow
                title={`Move ${branch} here, keep later changes as uncommitted`}
                consequence={`${branch} points at this commit; what the later commits changed stays in your files, not staged.`}
                disabled={moveBlock !== null}
                disabledTitle={moveBlock ?? undefined}
                onDo={() => reset("mixed")}
              />
              <GoBackRow
                title={`Move ${branch} here, keep later changes staged`}
                consequence={`${branch} points at this commit; what the later commits changed stays staged, ready to commit again.`}
                disabled={moveBlock !== null}
                disabledTitle={moveBlock ?? undefined}
                onDo={() => reset("soft")}
              />
              <GoBackRow
                title={`Move ${branch} here and drop everything after it`}
                consequence={`${branch} points at this commit and the later commits leave it. A backup branch keeps them; you confirm first.`}
                danger
                disabled={moveBlock !== null}
                disabledTitle={moveBlock ?? undefined}
                onDo={() => setHardConfirm(true)}
              />
            </ul>
          </div>
        )}

        {/* ---------------- start a branch ---------------- */}
        {mode === "branch" && (
          <div className="rounded-lg border border-zinc-700/50 bg-zinc-800/40 p-3 mb-4 min-w-0">
            <div className="flex items-start justify-between gap-2 mb-2">
              <div className="min-w-0">
                <h3 className="text-sm font-medium text-zinc-200">
                  Start a branch at <span className="font-mono">{detail.short}</span>
                </h3>
                <p className="text-xs text-zinc-500 mt-0.5">
                  Creates it and switches to it. Your uncommitted changes come along untouched.
                </p>
              </div>
              <button
                onClick={() => setMode(null)}
                aria-label="Close branch form"
                className="p-1 rounded-lg hover:bg-zinc-700 text-zinc-500 hover:text-zinc-200 transition-colors cursor-pointer shrink-0"
              >
                <X size={14} />
              </button>
            </div>
            <form onSubmit={createBranch} className="flex items-center gap-2 flex-wrap">
              <Input
                aria-label="Branch name"
                placeholder="new-branch-name"
                value={branchName}
                onChange={(e) => setBranchName(e.target.value)}
                autoFocus
                spellCheck={false}
                containerClassName="flex-1 min-w-[10rem]"
              />
              <Button type="submit" disabled={locked || !branchName.trim()} title={lockTitle}>
                <GitBranchPlus size={14} />
                Create
              </Button>
            </form>
          </div>
        )}

        <div className="grid sm:grid-cols-2 gap-3 text-xs mb-4">
          <div className="px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/40 min-w-0">
            <p className="text-zinc-500">Author</p>
            <p className="text-zinc-200 truncate">{detail.author_name}</p>
            <p className="text-zinc-500 font-mono truncate">{detail.author_email}</p>
            <p className="text-zinc-600 mt-1">{detail.author_date.replace("T", " ").slice(0, 19)}</p>
          </div>
          <div className="px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/40 min-w-0">
            <p className="text-zinc-500">
              Parent{detail.parents.length === 1 ? "" : "s"}
              {isMergeCommit && " — this is where two branches joined"}
            </p>
            {detail.parents.length === 0 && (
              <p className="text-zinc-400">none (root commit)</p>
            )}
            {detail.parents.map((p) => (
              <p key={p} className="text-zinc-300 font-mono truncate">{p.slice(0, 12)}</p>
            ))}
          </div>
        </div>

        {detail.body && (
          <pre className="p-3 rounded-lg bg-zinc-800/50 border border-zinc-700/40 text-xs text-zinc-300 whitespace-pre-wrap break-words mb-4 max-h-52 overflow-y-auto">
            {detail.body}
          </pre>
        )}

        <div className="flex items-center gap-2 mb-2">
          <FileText size={14} className="text-zinc-500" />
          <h3 className="text-sm font-medium text-zinc-300">
            {detail.files.length} file{detail.files.length === 1 ? "" : "s"} changed
          </h3>
          <span className="text-[11px] text-zinc-600">— click one to see its diff</span>
        </div>
        <div className={`space-y-1 pr-1 min-w-0 ${openFile ? "" : "max-h-64 overflow-y-auto"}`}>
          {detail.files.map((f) => {
            const open = openFile === f.path;
            return (
              <div key={f.path} className="min-w-0">
                <button
                  type="button"
                  onClick={() => setOpenFile(open ? null : f.path)}
                  aria-expanded={open}
                  className={`w-full flex items-center gap-3 px-3 py-1.5 rounded-lg border text-xs min-w-0 text-left transition-colors cursor-pointer ${
                    open
                      ? "bg-zinc-800/70 border-zinc-600"
                      : "bg-zinc-800/40 border-zinc-700/30 hover:border-zinc-600"
                  }`}
                >
                  <ChevronRight
                    size={12}
                    className={`shrink-0 text-zinc-500 transition-transform ${open ? "rotate-90" : ""}`}
                  />
                  <span className="font-mono text-zinc-300 truncate min-w-0 flex-1" title={f.path}>
                    {f.path}
                  </span>
                  {f.added === "-" ? (
                    // numstat prints "-" for both columns of a binary file
                    <span className="text-zinc-500 shrink-0">binary</span>
                  ) : (
                    <>
                      <span className="text-emerald-400 shrink-0 tabular-nums">+{f.added}</span>
                      <span className="text-red-400 shrink-0 tabular-nums">−{f.removed}</span>
                    </>
                  )}
                </button>
                {open && (
                  <div className="mt-1 mb-2 rounded-lg border border-zinc-700/40 bg-zinc-900/70 max-h-96 overflow-auto min-w-0">
                    <DiffView diff={fileDiff} loading={diffLoading} error={diffError} />
                  </div>
                )}
              </div>
            );
          })}
          {detail.files.length === 0 && (
            <p className="text-xs text-zinc-600">No file changes (empty or merge commit).</p>
          )}
        </div>
      </div>

      {/* ---------------- confirm: hard reset ---------------- */}
      <Modal
        open={hardConfirm}
        onClose={() => setHardConfirm(false)}
        title={`Move ${branch} back to ${detail.short}?`}
      >
        <div className="space-y-3">
          <p className="text-sm text-zinc-300">
            {dropped !== null && dropped > 0
              ? `The ${dropped} commit${dropped === 1 ? "" : "s"} after it leave${dropped === 1 ? "s" : ""} the branch.`
              : "Every commit after it leaves the branch."}
          </p>
          <p className="text-xs text-zinc-500">
            A backup branch ({"gitswitch-before-reset-<time>"}) keeps them; the undo command is shown afterwards.
          </p>
          {dirty && (
            <>
              <label className="flex items-start gap-2.5 cursor-pointer select-none">
                <Checkbox
                  aria-label="Stash uncommitted changes first"
                  checked={stash}
                  onChange={setStash}
                  className="mt-0.5"
                />
                <span className="text-sm text-zinc-200">Stash uncommitted changes first</span>
              </label>
              {!stash && (
                <p className="text-xs text-red-400">
                  Without stashing, your uncommitted changes are thrown away. That cannot be undone.
                </p>
              )}
            </>
          )}
          <div className="flex justify-end gap-2 flex-wrap pt-1">
            <Button variant="secondary" onClick={() => setHardConfirm(false)}>
              Keep things as they are
            </Button>
            <Button variant="danger" disabled={locked} onClick={() => reset("hard")}>
              Move {branch} back
            </Button>
          </div>
        </div>
      </Modal>

      {/* ---------------- confirm: revert ---------------- */}
      <Modal open={revertOpen} onClose={() => setRevertOpen(false)} title={`Revert ${detail.short}?`}>
        <div className="space-y-3">
          <p className="text-sm text-zinc-300">
            Makes a new commit on {branch} that undoes what this one did. Nothing is rewritten — the
            original stays in history.
          </p>
          <p className="text-xs text-zinc-500">If it conflicts, you resolve it on the Changes page.</p>
          {showMainline && (
            <div className="rounded-lg border border-zinc-700/50 bg-zinc-800/40 p-3 space-y-2">
              <p className="text-xs text-zinc-400">
                This is a merge, so pick the side to keep — the revert undoes what came in from the other one.
              </p>
              <div role="radiogroup" aria-label="Mainline" className="space-y-1.5">
                {mainlineParents.slice(0, 2).map((p, i) => (
                  <Radio
                    key={p.oid}
                    name="mainline"
                    checked={mainline === i + 1}
                    onChange={() => setMainline(i === 0 ? 1 : 2)}
                  >
                    Keep <span className="font-mono text-zinc-400">{p.short}</span>
                    {p.subject && <> {p.subject}</>}
                  </Radio>
                ))}
              </div>
            </div>
          )}
          <div className="flex justify-end gap-2 flex-wrap pt-1">
            <Button variant="secondary" onClick={() => setRevertOpen(false)}>
              Not now
            </Button>
            <Button variant="primary" disabled={locked} onClick={revert}>
              <RotateCcw size={14} />
              Revert
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}

function GoBackRow({
  title,
  consequence,
  danger = false,
  disabled,
  disabledTitle,
  onDo,
}: {
  title: string;
  consequence: string;
  danger?: boolean;
  disabled: boolean;
  disabledTitle?: string;
  onDo: () => void;
}) {
  return (
    <li
      aria-disabled={disabled || undefined}
      title={disabled ? disabledTitle : undefined}
      className={`flex items-center gap-3 px-3 py-2 rounded-lg border min-w-0 ${
        danger ? "border-red-500/25 bg-red-500/5" : "border-zinc-700/40 bg-zinc-800/50"
      } ${disabled ? "opacity-60" : ""}`}
    >
      <div className="min-w-0 flex-1">
        <p className={`text-sm break-words ${danger ? "text-red-300" : "text-zinc-200"}`}>{title}</p>
        <p className="text-xs text-zinc-500 break-words">{consequence}</p>
      </div>
      <Button
        size="sm"
        variant={danger ? "danger" : "secondary"}
        disabled={disabled}
        title={disabled ? disabledTitle : undefined}
        onClick={onDo}
      >
        Do it
      </Button>
    </li>
  );
}
