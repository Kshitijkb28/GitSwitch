import { useEffect, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, ExternalLink, KeyRound, Loader2, Trash2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "../../lib/api";
import { notifyRepoChange, type Deleted } from "../../lib/repoChanges";
import { Button } from "../Button";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { useToast } from "../Toast";
import { DeviceCodePanel } from "./DeviceCodePanel";
import { cleanError } from "./shared";
import { count, deleteBlock, listParts, loginOf, restoreNote, settingsUrl, typedMatches, useEscape } from "./repoAdmin";

type Props = {
  open: boolean;
  /** The account to act as ("gh:<login>" or "signin:<login>"). */
  account: string;
  owner: string;
  name: string;
  /** The folder on this computer, when known: it is checked for the commits GitHub has and it lacks. */
  repoPath?: string | null;
  onClose: () => void;
  /** Deleted on GitHub: "owner/name" as GitHub had it, GitHub's answer, and where it can be restored. */
  onDeleted?: (deleted: Deleted) => void;
};

/** Where the approval to delete stands. */
type Approval =
  | { phase: "explain" }
  | { phase: "starting" }
  | { phase: "code"; prompt: api.GrantPrompt }
  | { phase: "failed"; error: string };

/**
 * Delete a repository on GitHub, after saying what goes with it — and what
 * this folder would lose — and with its name typed. When GitHub wants the
 * person's approval first (the account's sign-in can't delete), the same
 * dialog asks for it with a code to enter on github.com; once approved,
 * Delete is pressed again: nothing is deleted by itself.
 */
export function DeleteRepoModal({ open, account, owner, name, repoPath = null, onClose, onDeleted }: Props) {
  const toast = useToast();
  const [facts, setFacts] = useState<api.RepoFacts | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Set while GitHub's approval is being asked for: the approval step replaces the confirm step. */
  const [approval, setApproval] = useState<Approval | null>(null);
  const [approved, setApproved] = useState(false);
  const ask = useRef(0);
  /** The approval being waited for; a newer one, or closing, makes an older answer moot. */
  const grant = useRef(0);
  /** An approval request or wait is with the backend: closing cancels it. */
  const granting = useRef(false);
  /**
   * A delete is on its way: a second click or Enter sends nothing more, and
   * the dialog stays busy until GitHub has answered, even when what it shows
   * changes meanwhile.
   */
  const sending = useRef(false);
  const input = useRef<HTMLInputElement>(null);
  const login = loginOf(account);
  const asked = `${owner}/${name}`;

  async function read() {
    const id = ++ask.current;
    setFacts(null);
    setLoadError(null);
    try {
      const f = await api.githubRepoFacts(account, owner, name, repoPath || null);
      if (id === ask.current) setFacts(f);
    } catch (e) {
      if (id === ask.current) setLoadError(cleanError(e));
    }
  }

  function stopWaiting() {
    grant.current++;
    if (granting.current) {
      granting.current = false;
      void api.githubDeleteGrantCancel().catch(() => undefined);
    }
  }

  useEffect(() => {
    // Closed, or another repository, account or folder: an approval being
    // waited for was for what the dialog showed before.
    stopWaiting();
    if (!open) {
      ask.current++;
      return;
    }
    setTyped("");
    setError(null);
    if (!sending.current) setBusy(false);
    setApproval(null);
    setApproved(false);
    void read();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, account, owner, name, repoPath]);

  // Leaving the page while GitHub's approval is awaited stops the wait.
  useEffect(() => () => stopWaiting(), []);

  const close = () => {
    if (busy) return;
    stopWaiting();
    onClose();
  };
  useEscape(open && !busy, close);

  const block = facts ? deleteBlock(facts.admin, login) : null;
  const matches = typedMatches(typed, owner, name);
  const ready = !!facts && !block && matches && !busy;

  async function remove() {
    if (!facts || !ready || sending.current) return;
    /** What the dialog showed when Delete was pressed: its answer isn't painted over another repository's. */
    const shown = ask.current;
    sending.current = true;
    setBusy(true);
    setError(null);
    try {
      const out = await api.githubDeleteRepo(account, owner, name, typed, facts.id);
      setBusy(false);
      if (out.outcome === "deleted") {
        // GitHub's answer as the backend words it: who can restore it, and from where.
        toast.success(out.message);
        // Where it can be restored, from the backend's answer: read from the same facts as its
        // message (GitHub's, just before the delete), so the note never contradicts it.
        const gone: Deleted = { fullName: facts.full_name || asked, message: out.message, restoreUrl: out.restore_url ?? null };
        notifyRepoChange({ kind: "deleted", ...gone });
        onDeleted?.(gone);
        // Closed only while it still shows this repository: one closed meanwhile, or
        // moved on to another repository or account, is left as it is.
        if (shown === ask.current) onClose();
        return;
      }
      // GitHub wants the person's approval first; nothing was sent.
      if (shown !== ask.current) return;
      setApproved(false);
      setApproval({ phase: "explain" });
    } catch (e) {
      if (shown === ask.current) setError(cleanError(e));
      setBusy(false);
    } finally {
      sending.current = false;
    }
  }

  async function getCode() {
    const id = ++grant.current;
    granting.current = true;
    setApproval({ phase: "starting" });
    let prompt: api.GrantPrompt;
    try {
      prompt = await api.githubDeleteGrantStart(account);
    } catch (e) {
      if (id !== grant.current) return;
      granting.current = false;
      setApproval({ phase: "failed", error: cleanError(e) });
      return;
    }
    if (id !== grant.current) {
      // Closed while GitHub was making the code: nobody is waiting for it.
      void api.githubDeleteGrantCancel().catch(() => undefined);
      return;
    }
    setApproval({ phase: "code", prompt });
    try {
      await api.githubDeleteGrantWait();
      if (id !== grant.current) return;
      granting.current = false;
      setApproval(null);
      setApproved(true);
      setError(null);
      // Back on the confirm step, the name still typed: Delete is the person's to press.
      window.setTimeout(() => input.current?.focus(), 0);
    } catch (e) {
      if (id !== grant.current) return;
      granting.current = false;
      setApproval({ phase: "failed", error: cleanError(e) });
    }
  }

  function backToConfirm() {
    stopWaiting();
    setApproval(null);
    window.setTimeout(() => input.current?.focus(), 0);
  }

  const title = approval ? "Approve deleting on GitHub" : "Delete this repository on GitHub?";
  const missing = facts?.local_checked ? facts.missing_here : [];

  return (
    // Coming back from the browser with the code entered, a click on the dimmed
    // area must not cancel the wait; nor lose a name typed.
    <Modal open={open} onClose={close} title={title} closeOnBackdrop={!approval && !typed}>
      <div className="space-y-4 text-sm" data-delete-dialog={approval ? `approve-${approval.phase}` : facts ? "confirm" : loadError ? "error" : "loading"}>
        {!facts && !loadError && (
          <p role="status" className="flex items-center gap-2 text-zinc-400">
            <Loader2 size={14} className="animate-spin text-emerald-400" />
            {repoPath ? `Reading ${asked} from GitHub and comparing it with this folder…` : `Reading ${asked} from GitHub…`}
          </p>
        )}
        {loadError && (
          <>
            <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
              {loadError}
            </p>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <button
                type="button"
                className="text-xs text-zinc-500 hover:text-zinc-300 hover:underline cursor-pointer inline-flex items-center gap-1"
                onClick={() => void openUrl(settingsUrl(owner, name)).catch(() => undefined)}
              >
                <ExternalLink size={12} /> Delete it on github.com instead
              </button>
              <div className="flex gap-2 ml-auto">
                <Button variant="secondary" onClick={close} autoFocus>
                  Cancel
                </Button>
                <Button onClick={() => void read()}>Try again</Button>
              </div>
            </div>
          </>
        )}

        {facts && approval && (
          <div className="space-y-4" aria-live="polite">
            {approval.phase === "explain" && (
              <>
                <p className="flex items-start gap-2 text-zinc-200">
                  <KeyRound size={15} className="mt-0.5 shrink-0 text-amber-300" />
                  <span className="min-w-0">Allow deleting as @{login}?</span>
                </p>
                <p className="text-zinc-400">
                  GitHub asks you to approve this once. GitSwitch keeps the permission in memory for 10 minutes and never saves it. Nothing is deleted yet.
                </p>
              </>
            )}
            {approval.phase === "starting" && (
              <p role="status" className="flex items-center gap-2 text-zinc-400">
                <Loader2 size={14} className="animate-spin text-emerald-400" /> Asking GitHub for a code…
              </p>
            )}
            {approval.phase === "code" && (
              <>
                <div className="space-y-1.5">
                  <p className="text-zinc-400">Enter this code on GitHub to allow GitSwitch to delete repositories as @{login}. Nothing is deleted yet.</p>
                  <p className="text-zinc-400" data-code-expiry>
                    When you have approved it, come back here and press Delete once more. The code works for{" "}
                    {count(Math.max(1, Math.round(approval.prompt.expires_in / 60)), "minute")}.
                  </p>
                </div>
                <DeviceCodePanel code={approval.prompt.user_code} url={approval.prompt.verification_uri} login={login} />
              </>
            )}
            {approval.phase === "failed" && (
              <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
                {approval.error}
              </p>
            )}
            <div className="flex flex-wrap items-center gap-2">
              <Button variant="ghost" onClick={backToConfirm} title="Back to the name typed, without approving">
                Back
              </Button>
              {/* The step's own button has the focus; Cancel is one Tab away. */}
              <div className="flex gap-2 ml-auto">
                <Button variant="secondary" onClick={close}>
                  Cancel
                </Button>
                {approval.phase === "explain" && (
                  <Button onClick={() => void getCode()} autoFocus>
                    <KeyRound size={14} /> Get a code
                  </Button>
                )}
                {approval.phase === "failed" && (
                  <Button onClick={() => void getCode()} autoFocus>
                    Try again
                  </Button>
                )}
              </div>
            </div>
          </div>
        )}

        {facts && !approval && (
          <>
            <p className="text-zinc-300">
              This deletes <span className="font-mono text-zinc-100 break-all">{facts.full_name}</span> on GitHub: its code, issues, pull requests, wiki, releases
              and settings. The folder on this computer is not touched.
            </p>
            {/* What else goes, what this folder would lose, and who could restore it: for the one who can delete it. */}
            {!block && facts.forks > 0 && (
              <p className="text-zinc-300" data-forks-line>
                {facts.private
                  ? `Its ${count(facts.forks, "fork")} ${facts.forks === 1 ? "is" : "are"} deleted too.`
                  : `Its ${count(facts.forks, "fork")} ${facts.forks === 1 ? "stays" : "stay"} on GitHub.`}
              </p>
            )}
            {!block && missing.length > 0 && (
              <p role="alert" className="flex items-start gap-2 px-3 py-2.5 rounded-lg bg-amber-500/15 border border-amber-500/40 text-amber-200" data-missing-here={missing.join(",")}>
                <AlertTriangle size={16} className="mt-0.5 shrink-0 text-amber-300" />
                <span className="min-w-0 break-words">
                  GitHub has commits on{" "}
                  {listParts(missing).map((p, i) =>
                    "name" in p ? (
                      <span key={i} className="font-mono text-amber-100">
                        {p.name}
                      </span>
                    ) : (
                      <span key={i}>{p.text}</span>
                    )
                  )}{" "}
                  that this folder doesn't have. Fetch them first, or they are lost.
                </span>
              </p>
            )}
            {!block && !repoPath && <p className="text-xs text-zinc-500">No folder on this computer was checked for its commits.</p>}
            {!block && repoPath && !facts.local_checked && (
              <p className="text-xs text-zinc-500">This folder could not be compared with GitHub: check it has every commit you want to keep.</p>
            )}
            {!block && repoPath && facts.local_checked && facts.branches_capped && (
              <p className="text-xs text-amber-300/90" data-branches-capped>
                GitHub has more than 1,000 branches: only the first 1,000 were compared with this folder.
              </p>
            )}
            {!block && (
              <p className="text-zinc-400" data-restore-note>
                {restoreNote(facts)}
              </p>
            )}

            {block ? (
              <p className="flex items-start gap-2 px-3 py-2 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-200 text-xs" data-delete-block>
                <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                <span className="min-w-0">{block}</span>
              </p>
            ) : (
              <>
                {facts.can_delete === false && !approved && (
                  <p className="text-xs text-zinc-400">
                    GitHub asks you to approve deleting once, as @{login}: after you press Delete, GitSwitch shows a code to enter on github.com.
                  </p>
                )}
                {approved && (
                  <p role="status" className="flex items-start gap-2 px-3 py-2 rounded-lg bg-emerald-500/10 border border-emerald-500/30 text-emerald-300 text-xs" data-approved>
                    <CheckCircle2 size={14} className="mt-0.5 shrink-0" />
                    <span className="min-w-0">Approved. Press Delete to delete {facts.full_name}.</span>
                  </p>
                )}
                <Input
                  ref={input}
                  autoFocus
                  label={`To confirm, type ${asked}`}
                  aria-label={`Type ${asked} to confirm`}
                  value={typed}
                  onChange={(e) => setTyped(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && ready) void remove();
                  }}
                  disabled={busy}
                  spellCheck={false}
                  autoComplete="off"
                  placeholder={asked}
                  className="font-mono text-sm"
                />
              </>
            )}

            {error && (
              <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
                {error}
              </p>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2">
              {/* GitHub shows a repository's settings to its admins only: for anyone else the link would lead nowhere. */}
              {!block ? (
                <button
                  type="button"
                  className="text-xs text-zinc-500 hover:text-zinc-300 hover:underline cursor-pointer inline-flex items-center gap-1"
                  onClick={() => void openUrl(settingsUrl(owner, name)).catch(() => undefined)}
                >
                  <ExternalLink size={12} /> Delete it on github.com instead
                </button>
              ) : (
                <span />
              )}
              <div className="flex gap-2 ml-auto">
                <Button variant="secondary" onClick={close} disabled={busy} autoFocus={!!block}>
                  {block ? "Close" : "Cancel"}
                </Button>
                {!block && (
                  <Button variant="danger" onClick={() => void remove()} disabled={!ready} data-confirm="delete">
                    {busy ? <Loader2 size={14} className="animate-spin" /> : <Trash2 size={14} />}
                    {busy ? "Deleting…" : "Delete this repository"}
                  </Button>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
