import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CircleDot,
  ExternalLink,
  FileText,
  GitCommit,
  GitMerge,
  Loader2,
  MessageSquare,
  MinusCircle,
  X,
  XCircle,
} from "lucide-react";
import * as api from "../../lib/api";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { RefreshButton } from "../RefreshButton";
import { Select } from "../Select";
import { Textarea } from "../Textarea";
import { useToast } from "../Toast";
import { DiffView, DiffViewToggle } from "../changes/DiffPanel";
import { usePersistedState } from "../../lib/persist";
import { PullStateIcon, StateLabel, ago, cleanError, openOnGitHub } from "./shared";

type Tab = "overview" | "files" | "commits";
type Method = "merge" | "squash" | "rebase";

const METHOD_LABEL: Record<Method, string> = { merge: "Create a merge commit", squash: "Squash and merge", rebase: "Rebase and merge" };
const METHOD_WORD: Record<Method, string> = { merge: "with a merge commit", squash: "by squashing", rebase: "by rebasing" };

/** What GitHub's mergeable state means, in words, and whether it stops a merge here. */
function mergeStatus(d: api.PullDetail): { tone: "ok" | "warn" | "bad" | "wait"; text: string } {
  if (d.state === "merged") return { tone: "ok", text: `Merged${d.merged_by ? ` by ${d.merged_by}` : ""} ${ago(d.merged_at)}.` };
  if (d.state === "closed") return { tone: "bad", text: "Closed without merging." };
  if (d.draft) return { tone: "warn", text: "This is a draft. Mark it ready for review on GitHub before merging." };
  if (d.mergeable === null) return { tone: "wait", text: "GitHub is still working out whether it can be merged…" };
  if (d.mergeable === false || d.mergeable_state === "dirty") return { tone: "bad", text: `This branch has conflicts with ${d.base_ref}. Resolve them (locally or on GitHub) before merging.` };
  switch (d.mergeable_state) {
    case "blocked":
      return { tone: "warn", text: "Blocked: the branch rules want approvals or passing checks first. GitHub decides when you press Merge." };
    case "behind":
      return { tone: "warn", text: `The branch is behind ${d.base_ref}. It can still be merged, unless the rules want it up to date.` };
    case "unstable":
      return { tone: "warn", text: "Some checks failed or are still running." };
    case "has_hooks":
    case "clean":
      return { tone: "ok", text: "No conflicts with the base branch: it can be merged." };
    default:
      return { tone: "ok", text: "No conflicts with the base branch." };
  }
}

function CheckIcon({ state }: { state: api.PullCheck["state"] }) {
  if (state === "success") return <CheckCircle2 size={14} className="text-emerald-400 shrink-0" />;
  if (state === "failure") return <XCircle size={14} className="text-red-400 shrink-0" />;
  if (state === "pending") return <CircleDot size={14} className="text-amber-400 shrink-0" />;
  return <MinusCircle size={14} className="text-zinc-500 shrink-0" />;
}

const REVIEW_WORD: Record<string, string> = { APPROVED: "approved", CHANGES_REQUESTED: "asked for changes", COMMENTED: "commented", DISMISSED: "review dismissed" };

/**
 * One pull request, to review and merge: what it changes (files with diffs,
 * commits), what GitHub says about it (conflicts, checks, reviews), a review
 * of your own, and Merge — with the commit you looked at, so nothing pushed
 * afterwards is merged unseen.
 */
export function PullPanel({
  account,
  repo,
  number,
  onClose,
  onChanged,
}: {
  account: string;
  /** "owner/name" */
  repo: string;
  number: number;
  onClose: () => void;
  /** Something changed on GitHub (merged, reviewed): lists should read again. */
  onChanged: () => void;
}) {
  const toast = useToast();
  const [owner, name] = repo.split("/");
  const [tab, setTab] = useState<Tab>("overview");
  const [detail, setDetail] = useState<api.PullDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [files, setFiles] = useState<api.PullFiles | null>(null);
  const [filesError, setFilesError] = useState<string | null>(null);
  const [openFiles, setOpenFiles] = useState<Set<string>>(new Set());
  const [commits, setCommits] = useState<api.PullCommit[] | null>(null);
  const [commitsError, setCommitsError] = useState<string | null>(null);
  const [reviewText, setReviewText] = useState("");
  const [sending, setSending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // Review and merge fold away on the Files tab: the diffs need the height.
  const [actionsOpen, setActionsOpen] = useState(true);
  const [savedMethod, setSavedMethod] = usePersistedState<Method>("pulls.mergeMethod", "merge");
  const [method, setMethod] = useState<Method>(savedMethod);
  const [title, setTitle] = useState("");
  const [deleteBranch, setDeleteBranch] = usePersistedState("pulls.deleteBranch", false);
  const [confirming, setConfirming] = useState(false);
  const [merging, setMerging] = useState(false);
  const recheck = useRef<number | null>(null);
  const ask = useRef(0);

  async function load(again = false) {
    const id = ++ask.current;
    setLoading(true);
    setError(null);
    try {
      const d = await api.githubPull(account, owner, name, number);
      if (id !== ask.current) return;
      setDetail(d);
      const m = d.merge_methods.includes(savedMethod) ? savedMethod : (d.merge_methods[0] ?? "merge");
      setMethod(m);
      setTitle((t) => t || `${d.title} (#${d.number})`);
      // GitHub works mergeability out in the background: ask once more shortly.
      if (d.state === "open" && d.mergeable === null && !again) {
        recheck.current = window.setTimeout(() => void load(true), 2000);
      }
    } catch (e) {
      if (id === ask.current) setError(cleanError(e));
    } finally {
      if (id === ask.current) setLoading(false);
    }
  }

  async function loadFiles() {
    setFilesError(null);
    try {
      setFiles(await api.githubPullFiles(account, owner, name, number));
    } catch (e) {
      setFilesError(cleanError(e));
    }
  }

  async function loadCommits() {
    setCommitsError(null);
    try {
      setCommits(await api.githubPullCommits(account, owner, name, number));
    } catch (e) {
      setCommitsError(cleanError(e));
    }
  }

  function refresh() {
    void load();
    if (files || filesError) void loadFiles();
    if (commits || commitsError) void loadCommits();
  }

  useEffect(() => {
    void load();
    return () => {
      if (recheck.current) window.clearTimeout(recheck.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account, repo, number]);

  useEffect(() => {
    setActionsOpen(tab !== "files");
    if (tab === "files" && !files && !filesError) void loadFiles();
    if (tab === "commits" && !commits && !commitsError) void loadCommits();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !confirming && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirming, onClose]);

  async function sendReview(event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
    setSending(event);
    setActionError(null);
    try {
      await api.githubPullReview(account, owner, name, number, event, reviewText);
      setReviewText("");
      toast.success(event === "APPROVE" ? `Approved #${number}` : event === "REQUEST_CHANGES" ? `Asked for changes on #${number}` : `Commented on #${number}`);
      onChanged();
      void load();
    } catch (e) {
      setActionError(cleanError(e));
    } finally {
      setSending(null);
    }
  }

  async function merge() {
    if (!detail) return;
    setMerging(true);
    setActionError(null);
    try {
      const out = await api.githubPullMerge(account, owner, name, number, method, detail.head_sha, method === "rebase" ? null : title, null, deleteBranch && detail.can_delete_branch);
      setSavedMethod(method);
      setConfirming(false);
      setTab("overview");
      toast.success(`Merged #${number} into ${detail.base_ref}${out.branch_note ? ` · ${out.branch_note.replace(/^Merged\. /, "")}` : ""}`);
      if (out.branch_deleted === false && out.branch_note) setActionError(out.branch_note);
      onChanged();
      void load();
    } catch (e) {
      setConfirming(false);
      setActionError(cleanError(e));
    } finally {
      setMerging(false);
    }
  }

  const d = detail;
  const own = !!d && d.author.toLowerCase() === d.viewer.toLowerCase();
  const status = d ? mergeStatus(d) : null;
  const blockMerge = !d
    ? "Loading…"
    : d.state !== "open"
      ? null
      : d.draft
        ? "A draft cannot be merged."
        : !d.can_push
          ? `@${d.viewer} cannot push to ${d.repo}, so it cannot merge. Act as another account.`
          : d.mergeable === null
            ? "GitHub is still checking it — Refresh in a moment."
            : d.mergeable === false
              ? "Resolve the conflicts first."
              : null;
  const methods = (d?.merge_methods.length ? d.merge_methods : ["merge"]) as Method[];

  return (
    <div className="fixed inset-0 z-40 flex justify-center" role="dialog" aria-label={`Pull request #${number}`}>
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />
      <div className="relative m-3 sm:m-6 w-full max-w-5xl flex flex-col rounded-xl border border-zinc-700/50 bg-zinc-900 shadow-2xl min-h-0">
        <div className="px-5 pt-4 pb-3 border-b border-zinc-800 space-y-2 shrink-0">
          <div className="flex items-start gap-3">
            {d && (
              <span className="mt-1">
                <PullStateIcon state={d.state} draft={d.draft} size={18} />
              </span>
            )}
            <div className="min-w-0 flex-1">
              <h2 className="text-lg font-semibold text-zinc-100 break-words">
                {d ? d.title : "Pull request"} <span className="text-zinc-500 font-normal">#{number}</span>
              </h2>
              {d && (
                <p className="text-xs text-zinc-400 flex items-center gap-2 flex-wrap mt-0.5">
                  <StateLabel state={d.state} draft={d.draft} />
                  <span>
                    <span className="text-zinc-200">{d.author}</span> wants to merge {d.commits} commit{d.commits === 1 ? "" : "s"} into{" "}
                    <span className="font-mono text-zinc-200">{d.base_ref}</span> from{" "}
                    <span className="font-mono text-zinc-200">{d.head_repo && d.head_repo.toLowerCase() !== d.repo.toLowerCase() ? `${d.head_repo.split("/")[0]}:` : ""}{d.head_ref}</span>
                  </span>
                  <span className="text-emerald-400 tabular-nums">+{d.additions}</span>
                  <span className="text-red-400 tabular-nums">−{d.deletions}</span>
                  <span className="text-zinc-500 font-mono">{d.repo}</span>
                </p>
              )}
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <RefreshButton onClick={refresh} busy={loading} title="Read this pull request again" />
              {d && (
                <Button variant="ghost" size="sm" onClick={() => openOnGitHub(d.html_url)} title="Open on GitHub">
                  <ExternalLink size={14} />
                  <span className="hidden sm:inline">GitHub</span>
                </Button>
              )}
              <button onClick={onClose} aria-label="Close pull request" className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 cursor-pointer">
                <X size={18} />
              </button>
            </div>
          </div>
          <div className="flex gap-1" role="tablist">
            {(["overview", "files", "commits"] as const).map((t) => (
              <button
                key={t}
                type="button"
                role="tab"
                aria-selected={tab === t}
                onClick={() => setTab(t)}
                className={`px-3 py-1.5 text-sm rounded-md cursor-pointer transition-colors ${tab === t ? "bg-zinc-800 text-emerald-400" : "text-zinc-400 hover:text-zinc-200"}`}
              >
                {t === "overview" ? "Overview" : t === "files" ? `Files${d ? ` (${d.changed_files})` : ""}` : `Commits${d ? ` (${d.commits})` : ""}`}
              </button>
            ))}
          </div>
        </div>

        <div className="flex-1 overflow-y-auto min-h-0 px-5 py-4 space-y-4 text-sm">
          {error && (
            <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
              {error}
            </p>
          )}
          {!d && loading && (
            <p className="flex items-center gap-2 text-zinc-400" role="status">
              <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading the pull request…
            </p>
          )}

          {d && tab === "overview" && (
            <>
              {status && (
                <div
                  data-merge-status={status.tone}
                  className={`flex items-start gap-2 px-3 py-2.5 rounded-lg border ${
                    status.tone === "ok"
                      ? "bg-emerald-500/10 border-emerald-500/30 text-emerald-300"
                      : status.tone === "bad"
                        ? "bg-red-500/10 border-red-500/30 text-red-300"
                        : status.tone === "wait"
                          ? "bg-zinc-800/60 border-zinc-700 text-zinc-300"
                          : "bg-amber-500/10 border-amber-500/30 text-amber-300"
                  }`}
                >
                  {status.tone === "wait" ? <Loader2 size={15} className="animate-spin shrink-0 mt-0.5" /> : status.tone === "ok" ? <CheckCircle2 size={15} className="shrink-0 mt-0.5" /> : <AlertTriangle size={15} className="shrink-0 mt-0.5" />}
                  <span>{status.text}</span>
                </div>
              )}

              {d.checks.length > 0 && (
                <section>
                  <h3 className="text-xs uppercase tracking-wider text-zinc-500 mb-1.5">
                    Checks · {d.checks.filter((c) => c.state === "success").length} of {d.checks.length} passed
                  </h3>
                  <ul className="space-y-1">
                    {d.checks.map((c, i) => (
                      <li key={`${c.name}-${i}`} className="flex items-center gap-2 text-xs text-zinc-300">
                        <CheckIcon state={c.state} />
                        <span className="truncate">{c.name}</span>
                        <span className="text-zinc-500">{c.state}</span>
                      </li>
                    ))}
                  </ul>
                </section>
              )}

              <section>
                <h3 className="text-xs uppercase tracking-wider text-zinc-500 mb-1.5">Reviews</h3>
                {d.reviews.length === 0 ? (
                  <p className="text-xs text-zinc-500">No reviews yet.</p>
                ) : (
                  <ul className="space-y-1">
                    {d.reviews.map((r) => (
                      <li key={r.login} className="flex items-center gap-2 text-xs text-zinc-300">
                        {r.state === "APPROVED" ? <CheckCircle2 size={14} className="text-emerald-400" /> : r.state === "CHANGES_REQUESTED" ? <XCircle size={14} className="text-red-400" /> : <MessageSquare size={14} className="text-zinc-500" />}
                        <span className="text-zinc-200">{r.login}</span> {REVIEW_WORD[r.state] ?? r.state.toLowerCase()} <span className="text-zinc-500">{ago(r.submitted_at)}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              {d.notes.length > 0 && (
                <ul className="text-xs text-zinc-500 space-y-0.5">
                  {d.notes.map((n) => (
                    <li key={n}>{n}</li>
                  ))}
                </ul>
              )}

              <section>
                <h3 className="text-xs uppercase tracking-wider text-zinc-500 mb-1.5">Description</h3>
                {d.body.trim() ? (
                  <pre className="p-3 rounded-lg bg-zinc-800/50 border border-zinc-700/40 text-xs text-zinc-300 whitespace-pre-wrap break-words max-h-72 overflow-y-auto font-sans">{d.body}</pre>
                ) : (
                  <p className="text-xs text-zinc-500">No description.</p>
                )}
              </section>
            </>
          )}

          {d && tab === "files" && (
            <>
              {filesError && (
                <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs">
                  {filesError}
                </p>
              )}
              {!files && !filesError && (
                <p className="flex items-center gap-2 text-zinc-400" role="status">
                  <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading the changed files…
                </p>
              )}
              {files && (
                <div className="space-y-1">
                  <div className="flex items-center gap-3 mb-2 flex-wrap">
                    <FileText size={14} className="text-zinc-500" />
                    <span className="text-zinc-300">
                      {files.files.length} file{files.files.length === 1 ? "" : "s"} changed{files.truncated ? " (GitHub lists the first 3000)" : ""}
                    </span>
                    <button type="button" className="text-xs text-emerald-400 hover:text-emerald-300 cursor-pointer" onClick={() => setOpenFiles(new Set(files.files.map((f) => f.path)))}>
                      Expand all
                    </button>
                    <button type="button" className="text-xs text-zinc-400 hover:text-zinc-200 cursor-pointer" onClick={() => setOpenFiles(new Set())}>
                      Collapse all
                    </button>
                    <span className="ml-auto">
                      <DiffViewToggle />
                    </span>
                  </div>
                  {files.files.map((f) => {
                    const open = openFiles.has(f.path);
                    return (
                      <div key={f.path} className="min-w-0" data-pull-file={f.path}>
                        <button
                          type="button"
                          aria-expanded={open}
                          onClick={() =>
                            setOpenFiles((s) => {
                              const n = new Set(s);
                              if (open) n.delete(f.path);
                              else n.add(f.path);
                              return n;
                            })
                          }
                          className={`w-full flex items-center gap-3 px-3 py-1.5 rounded-lg border text-xs min-w-0 text-left cursor-pointer ${open ? "bg-zinc-800/70 border-zinc-600" : "bg-zinc-800/40 border-zinc-700/30 hover:border-zinc-600"}`}
                        >
                          <ChevronRight size={12} className={`shrink-0 text-zinc-500 transition-transform ${open ? "rotate-90" : ""}`} />
                          <span className="font-mono text-zinc-300 truncate min-w-0 flex-1" title={f.path}>
                            {f.previous_path ? `${f.previous_path} → ${f.path}` : f.path}
                          </span>
                          <span className="text-zinc-500 shrink-0">{f.status}</span>
                          <span className="text-emerald-400 shrink-0 tabular-nums">+{f.additions}</span>
                          <span className="text-red-400 shrink-0 tabular-nums">−{f.deletions}</span>
                        </button>
                        {open && (
                          <div className="mt-1 mb-2 rounded-lg border border-zinc-700/40 bg-zinc-900/70 min-w-0" data-pull-diff>
                            <DiffView diff={f.diff} loading={false} error={null} toggle={false} />
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </>
          )}

          {d && tab === "commits" && (
            <>
              {commitsError && (
                <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs">
                  {commitsError}
                </p>
              )}
              {!commits && !commitsError && (
                <p className="flex items-center gap-2 text-zinc-400" role="status">
                  <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading the commits…
                </p>
              )}
              {commits && (
                <ul className="space-y-1">
                  {commits.map((c) => (
                    <li key={c.sha} className="flex items-center gap-3 px-3 py-1.5 rounded-lg bg-zinc-800/40 border border-zinc-700/30 text-xs min-w-0">
                      <GitCommit size={13} className="text-zinc-500 shrink-0" />
                      <span className="text-zinc-200 truncate flex-1 min-w-0">{c.title}</span>
                      <span className="text-zinc-500 shrink-0">{c.author}</span>
                      <span className="text-zinc-500 shrink-0 hidden sm:inline">{ago(c.date)}</span>
                      <span className="font-mono text-zinc-500 shrink-0">{c.sha.slice(0, 7)}</span>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>

        {d && d.state === "open" && !(actionsOpen || actionError) && (
          <div className="border-t border-zinc-800 px-5 py-2 shrink-0 flex items-center gap-3" data-pull-actions="folded">
            <button
              type="button"
              onClick={() => setActionsOpen(true)}
              className="inline-flex items-center gap-1.5 text-sm text-zinc-300 hover:text-zinc-100 cursor-pointer"
              aria-expanded={false}
            >
              <ChevronUp size={15} />
              Review or merge
            </button>
            <span className="text-xs text-zinc-500 truncate">as @{d.viewer}, into {d.base_ref}</span>
          </div>
        )}
        {d && d.state === "open" && (actionsOpen || actionError) && (
          <div className="border-t border-zinc-800 px-5 py-3 space-y-3 shrink-0 max-h-[45vh] overflow-y-auto" data-pull-actions="open">
            {tab === "files" && (
              <button
                type="button"
                onClick={() => {
                  setActionsOpen(false);
                  setActionError(null);
                }}
                className="inline-flex items-center gap-1.5 text-xs text-zinc-400 hover:text-zinc-200 cursor-pointer"
                aria-expanded={true}
              >
                <ChevronDown size={14} />
                Hide review and merge
              </button>
            )}
            {actionError && (
              <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
                {actionError}
              </p>
            )}
            <div className="grid gap-3 lg:grid-cols-2">
              <section className="space-y-2" aria-label="Review">
                <h3 className="text-xs uppercase tracking-wider text-zinc-500">Your review, as @{d.viewer}</h3>
                <Textarea value={reviewText} onChange={(e) => setReviewText(e.target.value)} placeholder={own ? "Leave a comment" : "Leave a comment, or say what should change"} rows={2} aria-label="Review comment" />
                <div className="flex gap-2 flex-wrap">
                  {!own && (
                    <Button size="sm" variant="secondary" onClick={() => void sendReview("APPROVE")} disabled={!!sending}>
                      {sending === "APPROVE" ? <Loader2 size={14} className="animate-spin" /> : <CheckCircle2 size={14} />}
                      Approve
                    </Button>
                  )}
                  {!own && (
                    <Button size="sm" variant="secondary" onClick={() => void sendReview("REQUEST_CHANGES")} disabled={!!sending || !reviewText.trim()} title={reviewText.trim() ? "" : "Write what should change first"}>
                      {sending === "REQUEST_CHANGES" ? <Loader2 size={14} className="animate-spin" /> : <XCircle size={14} />}
                      Request changes
                    </Button>
                  )}
                  <Button size="sm" variant="secondary" onClick={() => void sendReview("COMMENT")} disabled={!!sending || !reviewText.trim()}>
                    {sending === "COMMENT" ? <Loader2 size={14} className="animate-spin" /> : <MessageSquare size={14} />}
                    Comment
                  </Button>
                </div>
                {own && <p className="text-xs text-zinc-500">It is @{d.viewer}'s own pull request: GitHub only lets them comment.</p>}
              </section>

              <section className="space-y-2" aria-label="Merge">
                <h3 className="text-xs uppercase tracking-wider text-zinc-500">Merge into {d.base_ref}</h3>
                <div className="flex gap-2 flex-wrap items-center">
                  <div className="w-56">
                    <Select value={method} onChange={(v) => setMethod(v as Method)} options={methods.map((m) => ({ value: m, label: METHOD_LABEL[m] }))} disabled={merging} />
                  </div>
                  <Button onClick={() => setConfirming(true)} disabled={!!blockMerge || merging}>
                    <GitMerge size={15} />
                    Merge
                  </Button>
                </div>
                {method !== "rebase" && <Input value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Commit title" placeholder="Commit title" className="!py-1.5 text-sm" />}
                {d.can_delete_branch && (
                  <label className="flex items-center gap-2 text-xs text-zinc-300 cursor-pointer select-none">
                    <Checkbox checked={deleteBranch} onChange={setDeleteBranch} aria-label="Delete the branch after merging" />
                    Delete <span className="font-mono">{d.head_ref}</span> after merging
                  </label>
                )}
                {blockMerge && <p className="text-xs text-amber-300/90">{blockMerge}</p>}
                {!d.merge_methods_known && <p className="text-xs text-zinc-500">GitHub tells only admins which merge methods this repository allows; it refuses one it doesn't.</p>}
              </section>
            </div>
          </div>
        )}
      </div>

      <Modal open={confirming} onClose={() => !merging && setConfirming(false)} title={`Merge pull request #${number}?`}>
        {d && (
          <div className="space-y-3 text-sm">
            <p className="text-zinc-300">
              <span className="font-mono">{d.head_ref}</span> goes into <span className="font-mono">{d.base_ref}</span> of {d.repo} {METHOD_WORD[method]}, as @{d.viewer}.
            </p>
            <p className="text-zinc-400 text-xs">
              Only the commits you reviewed are merged (up to {d.head_sha.slice(0, 7)}): if someone pushed since, GitHub refuses and nothing changes.
              {deleteBranch && d.can_delete_branch ? ` The branch ${d.head_ref} is deleted afterwards.` : ""}
            </p>
            <div className="flex justify-end gap-2 pt-1">
              <Button variant="secondary" onClick={() => setConfirming(false)} disabled={merging}>
                Cancel
              </Button>
              <Button onClick={() => void merge()} disabled={merging}>
                {merging ? <Loader2 size={15} className="animate-spin" /> : <GitMerge size={15} />}
                Merge #{number}
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
