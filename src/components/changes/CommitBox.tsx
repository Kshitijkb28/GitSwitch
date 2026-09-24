import { GitCommit, ShieldCheck, ShieldAlert, PenLine, AlertTriangle, GitBranch } from "lucide-react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Textarea } from "../Textarea";
import type { RepoStatus } from "../../lib/api";

interface Props {
  status: RepoStatus;
  message: string;
  onMessage: (m: string) => void;
  amend: boolean;
  onAmend: (v: boolean) => void;
  busy: boolean;
  onCommit: () => void;
  /** Offered next to the detached-HEAD blocker: opens the branch picker. */
  onSwitchBranch?: () => void;
  /** A submodule's path: the button says where the commit lands. */
  scopeLabel?: string;
}

/**
 * The one thing that must be true before a commit — and the reason this page
 * exists — is that it lands as the right person. So the identity is stated
 * first, in the same words the commit will carry, and it comes from git itself
 * rather than from what the profile intends.
 */
export function commitBlocker(status: RepoStatus, message: string, amend: boolean): string | null {
  if (status.conflicted_count > 0) {
    return `${status.conflicted_count} file${status.conflicted_count === 1 ? "" : "s"} still have conflicts. Resolve and stage them first.`;
  }
  if (status.detached) {
    return "You're not on a branch (detached HEAD). Switch to a branch before committing.";
  }
  if (!status.identity.matches_profile) {
    const expected = status.identity.profile_email ?? "the profile's email";
    return `This folder should commit as ${expected}, but git is set to ${status.identity.email || "nothing"}.`;
  }
  if (amend && !status.can_amend) {
    return "That commit is already on the remote — amending it would need a force-push.";
  }
  // A merge is finished by one commit even when the resolution staged nothing.
  const finishingMerge = status.operation?.kind === "merge";
  if (!amend && status.staged_count === 0 && !finishingMerge) {
    return "Nothing is staged.";
  }
  // After the staged check, so an untouched repo reports the real blocker.
  if (!message.trim()) return "A commit needs a message.";
  return null;
}

export function CommitBox({
  status,
  message,
  onMessage,
  amend,
  onAmend,
  busy,
  onCommit,
  onSwitchBranch,
  scopeLabel,
}: Props) {
  const blocker = commitBlocker(status, message, amend);
  const id = status.identity;
  const finishingMerge = status.operation?.kind === "merge";
  const firstLine = message.split("\n")[0] ?? "";
  const inside = scopeLabel ? ` in ${scopeLabel}` : "";

  return (
    <div className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-3 space-y-3">
      <div className="flex items-start gap-2">
        <GitCommit size={15} className="shrink-0 mt-0.5 text-zinc-400" />
        <div className="min-w-0 flex-1">
          <p className="text-sm text-zinc-200">
            Committing as{" "}
            <span className="font-medium text-zinc-100">{id.name || "?"}</span>{" "}
            <span className="font-mono text-xs text-zinc-400">&lt;{id.email || "unset"}&gt;</span>
            {scopeLabel && (
              <>
                {" "}
                <span className="text-zinc-400">
                  inside <span className="font-mono text-xs text-zinc-300">{scopeLabel}</span>
                </span>
              </>
            )}
          </p>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 mt-1 text-xs">
            {id.profile_name && (
              <span className="text-zinc-500">
                profile <span className="text-zinc-400">{id.profile_name}</span>
              </span>
            )}
            {id.email_scope === "local" && (
              <span className="text-amber-400/90" title={id.email_origin ?? undefined}>
                set by this repo's own .git/config
              </span>
            )}
            <span className={id.signing_on ? "text-emerald-400" : "text-zinc-500"}>
              {id.signing_on ? "signing on" : "not signed"}
            </span>
            {id.guard !== "none" && (
              <span className="text-zinc-500 inline-flex items-center gap-1">
                {id.guard === "gitswitch" ? (
                  <ShieldCheck size={11} className="text-emerald-400" />
                ) : (
                  <ShieldAlert size={11} className="text-amber-400" />
                )}
                {id.guard === "gitswitch" ? "guard on" : "third-party pre-commit hook"}
              </span>
            )}
          </div>
        </div>
      </div>

      {!id.matches_profile && (
        <div className="flex items-start gap-2 rounded-lg bg-amber-500/10 border border-amber-500/30 px-3 py-2">
          <AlertTriangle size={14} className="shrink-0 mt-0.5 text-amber-400" />
          <p className="text-xs text-amber-200/90 leading-relaxed">
            This commit would carry the wrong identity, so it is blocked. Expected{" "}
            <span className="font-mono">{id.profile_email}</span>
            {id.email_scope === "local" && " — this repo's .git/config overrides the profile"}.
          </p>
        </div>
      )}

      {finishingMerge && (
        <p className="text-xs text-sky-300/90">
          This commit finishes the merge that's in progress.
        </p>
      )}

      <Textarea
        value={message}
        onChange={(e) => onMessage(e.target.value)}
        placeholder={amend ? "New message for the last commit…" : "What changed, and why?"}
        rows={3}
        hint={
          firstLine.length > 72
            ? `First line is ${firstLine.length} characters — most tools wrap at 72.`
            : undefined
        }
      />

      <div className="flex items-center justify-between gap-3 flex-wrap">
        <label
          className={`flex items-center gap-2 text-xs ${
            status.can_amend ? "text-zinc-300 cursor-pointer" : "text-zinc-600 cursor-not-allowed"
          }`}
          title={
            status.can_amend
              ? "Replace the last commit instead of adding a new one"
              : "Amending a pushed commit would need a force-push, which GitSwitch never does"
          }
        >
          <Checkbox
            checked={amend}
            onChange={onAmend}
            disabled={!status.can_amend}
            aria-label="Amend the last commit"
          />
          {status.can_amend
            ? `Amend "${(status.head_subject ?? "").slice(0, 32)}${(status.head_subject ?? "").length > 32 ? "…" : ""}"`
            : "Amend (the last commit is already pushed)"}
        </label>
        <div className="flex items-center gap-2 flex-wrap">
          {blocker && <span className="text-xs text-zinc-500 max-w-xs">{blocker}</span>}
          {status.detached && onSwitchBranch && (
            <Button size="sm" variant="ghost" disabled={busy} onClick={onSwitchBranch}>
              <GitBranch size={13} />
              Choose a branch
            </Button>
          )}
          <Button size="sm" disabled={busy || blocker !== null} onClick={onCommit}>
            <PenLine size={14} />
            {amend ? `Amend commit${inside}` : `Commit${inside}`}
          </Button>
        </div>
      </div>
    </div>
  );
}
