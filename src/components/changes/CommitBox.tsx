import { GitCommit, ShieldCheck, ShieldAlert, PenLine, AlertTriangle, GitBranch } from "lucide-react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Textarea } from "../Textarea";
import type { ReactNode } from "react";
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
  /** Switch this repository's identity guard: on, off, or rewritten with the profile's email. */
  onGuard?: (action: GuardAction) => void;
  guardBusy?: boolean;
}

export type GuardAction = "on" | "off" | "update";

/** A GitSwitch guard that expects another email than git commits with: it would refuse the commit. */
export function staleGuard(id: RepoStatus["identity"]): boolean {
  return (
    id.guard === "gitswitch" &&
    !!id.guard_email &&
    id.guard_email.trim().toLowerCase() !== (id.email ?? "").trim().toLowerCase()
  );
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
  // A rebase detaches HEAD for its whole duration, and committing is how a
  // stopped step can be finished — so only a bare detached HEAD is the blocker.
  if (status.detached && !status.operation) {
    return "You're not on a branch (detached HEAD). Switch to a branch before committing.";
  }
  if (!status.identity.matches_profile) {
    const expected = status.identity.profile_email ?? "the profile's email";
    return `This folder should commit as ${expected}, but git is set to ${status.identity.email || "nothing"}.`;
  }
  if (staleGuard(status.identity)) {
    return `This repository's identity guard still expects ${status.identity.guard_email} — update it or turn it off first.`;
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
  onGuard,
  guardBusy,
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
                {id.guard === "gitswitch" && !staleGuard(id) ? (
                  <ShieldCheck size={11} className="text-emerald-400" />
                ) : (
                  <ShieldAlert size={11} className="text-amber-400" />
                )}
                {id.guard === "gitswitch"
                  ? staleGuard(id)
                    ? <span>guard expects <span className="font-mono">{id.guard_email}</span></span>
                    : "guard on"
                  : "third-party pre-commit hook"}
              </span>
            )}
            {onGuard && (id.profile_email || id.guard === "gitswitch") && (
              <span className="inline-flex items-center gap-2">
                {id.guard === "gitswitch" && staleGuard(id) && id.profile_email && (
                  <GuardButton disabled={guardBusy} onClick={() => onGuard("update")} title={`Rewrite the guard to expect ${id.profile_email}`}>
                    Update guard
                  </GuardButton>
                )}
                {id.guard === "gitswitch" ? (
                  <GuardButton disabled={guardBusy} onClick={() => onGuard("off")} title="Remove GitSwitch's pre-commit identity check from this repository">
                    Turn off guard
                  </GuardButton>
                ) : (
                  id.profile_email && (
                    <GuardButton
                      disabled={guardBusy}
                      onClick={() => onGuard("on")}
                      title={
                        id.guard === "foreign"
                          ? `Refuse commits not made as ${id.profile_email}. This repository's own pre-commit hook is kept and still runs.`
                          : `Refuse commits not made as ${id.profile_email}, from the app, a terminal or an editor`
                      }
                    >
                      Turn on guard
                    </GuardButton>
                  )
                )}
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
          {status.detached && !status.operation && onSwitchBranch && (
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

function GuardButton({ children, disabled, onClick, title }: { children: ReactNode; disabled?: boolean; onClick: () => void; title: string }) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      title={title}
      className="text-emerald-400 hover:text-emerald-300 disabled:text-zinc-600 disabled:cursor-not-allowed cursor-pointer"
    >
      {children}
    </button>
  );
}
