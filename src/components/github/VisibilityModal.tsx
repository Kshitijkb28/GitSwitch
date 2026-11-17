import { ReactNode, useEffect, useRef, useState } from "react";
import { AlertTriangle, Building2, ExternalLink, Globe, Info, Loader2, Lock } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "../../lib/api";
import { notifyRepoChange } from "../../lib/repoChanges";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { useToast } from "../Toast";
import { cleanError } from "./shared";
import { count, loginOf, pickedBehind, settingsUrl, starsAndWatchers, typedMatches, useEscape, visibilityBlock, type Want } from "./repoAdmin";

type Props = {
  open: boolean;
  /** The account to act as ("gh:<login>" or "signin:<login>"). */
  account: string;
  owner: string;
  name: string;
  /**
   * What the person picked: "Make public…" or "Make private…". The dialog
   * never turns it around. When GitHub has it that way already (the row or
   * card was behind), it says so and offers nothing to press.
   */
  want: Want;
  onClose: () => void;
  /** Changed on GitHub, or found changed there already: what GitHub has now. */
  onChanged?: (facts: api.RepoFacts) => void;
};

/** Who can see it now, in one line, with its icon. */
export function VisibilityNow({ facts }: { facts: api.RepoFacts }) {
  const repo = <span className="font-mono text-zinc-100 break-all">{facts.full_name}</span>;
  const [icon, text]: [ReactNode, ReactNode] =
    facts.visibility === "public"
      ? [<Globe size={15} className="text-sky-300" />, <>is public now: anyone on the internet can see it.</>]
      : facts.visibility === "internal"
        ? [<Building2 size={15} className="text-violet-300" />, <>is internal now: everyone in {facts.owner}'s enterprise can see it.</>]
        : [
            <Lock size={15} className="text-amber-300" />,
            facts.owner_kind === "org" ? <>is private now: only the people {facts.owner} gives access to can see it.</> : <>is private now: only you and the people you invite can see it.</>,
          ];
  return (
    <p className="flex items-start gap-2 text-zinc-300" data-visibility-now={facts.visibility}>
      <span className="mt-0.5 shrink-0">{icon}</span>
      <span className="min-w-0">
        {repo} {text}
      </span>
    </p>
  );
}

/** A list of what a change does, one plain sentence each. */
function Consequences({ lines }: { lines: { text: string; warn?: boolean }[] }) {
  return (
    <div>
      <p className="text-xs uppercase tracking-wider text-zinc-500 mb-1.5">What changes</p>
      <ul className="space-y-1.5" aria-label="What changes">
        {lines.map((l) => (
          <li key={l.text} className={`flex items-start gap-2 ${l.warn ? "text-amber-200" : "text-zinc-300"}`}>
            {l.warn ? (
              <AlertTriangle size={14} className="mt-0.5 shrink-0 text-amber-300" />
            ) : (
              <span aria-hidden="true" className="mt-[7px] h-1.5 w-1.5 rounded-full bg-zinc-500 shrink-0" />
            )}
            <span className="min-w-0">{l.text}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * Make a repository private or public on GitHub, as the person picked. It
 * reads the repository first and shows only what applies to it — its stars
 * and watchers, its forks, its Pages site — then asks for a word that fits
 * the step: typing its name to make it public, a tick when making it private
 * loses stars. While GitHub is asked, the dialog can't be closed.
 */
export function VisibilityModal({ open, account, owner, name, want, onClose, onChanged }: Props) {
  const toast = useToast();
  const [facts, setFacts] = useState<api.RepoFacts | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** What was picked, as it was when the dialog opened: a row or card that corrects itself meanwhile doesn't turn it around. */
  const [asked, setAsked] = useState<Want>(want);
  const [typed, setTyped] = useState("");
  const [understood, setUnderstood] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ask = useRef(0);
  /** A request is on its way: a second click or Enter sends nothing more. */
  const sending = useRef(false);
  const login = loginOf(account);

  async function read(picked: Want) {
    const id = ++ask.current;
    setFacts(null);
    setLoadError(null);
    try {
      const f = await api.githubRepoFacts(account, owner, name, null);
      if (id !== ask.current) return;
      setFacts(f);
      // The page offered this from what it read earlier: GitHub has moved on
      // (or the repository is internal). The row or card is told what GitHub
      // has now, so it offers the right step next time.
      if (pickedBehind(f, picked)) {
        notifyRepoChange({ kind: "visibility", fullName: f.full_name, private: f.private, visibility: f.visibility });
        onChanged?.(f);
      }
    } catch (e) {
      if (id === ask.current) setLoadError(cleanError(e));
    }
  }

  useEffect(() => {
    if (!open) {
      ask.current++;
      setFacts(null);
      setLoadError(null);
      return;
    }
    setAsked(want);
    setTyped("");
    setUnderstood(false);
    setError(null);
    // A change still on its way keeps the dialog busy until GitHub has answered it.
    if (!sending.current) setBusy(false);
    void read(want);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, account, owner, name]);

  const close = () => {
    if (!busy) onClose();
  };
  useEscape(open && !busy, onClose);

  const behind = facts ? pickedBehind(facts, asked) : null;
  const block = facts && !behind ? visibilityBlock(facts, login) : null;
  const sw = facts ? starsAndWatchers(facts) : null;
  /** "Its 1 star is…", "Its 3 stars are…", "Its 1 star and 1 watcher are…". */
  const swGo = facts && facts.stars + facts.watchers === 1 ? "is removed for good" : "are removed for good";
  const matches = typedMatches(typed, owner, name);
  const ready = !!facts && !behind && !block && !busy && (asked === "public" ? matches : !sw || understood);

  async function submit() {
    if (!facts || !ready || sending.current) return;
    /** What the dialog showed when it was pressed: its answer isn't painted over another repository or account. */
    const shown = ask.current;
    sending.current = true;
    setBusy(true);
    setError(null);
    try {
      const after = await api.githubSetVisibility(account, owner, name, asked === "private", facts.id);
      // It happened on GitHub: said, and every listing told, whatever the dialog shows by now.
      toast.success(`${after.full_name} is now ${after.private ? "private" : "public"}`);
      notifyRepoChange({ kind: "visibility", fullName: after.full_name, private: after.private, visibility: after.visibility });
      setBusy(false);
      // The card is told and the dialog closed only while it still shows this repository:
      // one closed meanwhile, or moved on to another repository or account, is left as it is.
      if (shown !== ask.current) return;
      onChanged?.(after);
      onClose();
    } catch (e) {
      if (shown === ask.current) setError(cleanError(e));
      setBusy(false);
    } finally {
      sending.current = false;
    }
  }

  const forks = facts && facts.forks > 0 ? facts.forks : 0;
  const forkWords = (stay: "public" | "private") =>
    `Its ${count(forks, "fork")} ${forks === 1 ? "stays" : "stay"} ${stay} and ${forks === 1 ? "is" : "are"} split off from it.`;
  /**
   * An internal repository made private: GitHub's "Setting repository
   * visibility" says its current forks "will be detached from this
   * repository" — split off, as a public one's are. Who can see them then is
   * left unsaid: the page words it as for a public repository ("remain
   * public"), which an internal one's forks never were.
   */
  const splitOff = `Its ${count(forks, "fork")} ${forks === 1 ? "is" : "are"} split off from it.`;
  /** Made private, something is lost for good: its stars and watchers. */
  const losesForGood = !!sw;
  const lines: { text: string; warn?: boolean }[] = !facts
    ? []
    : asked === "public"
      ? [
          { text: "Anyone on the internet will be able to see its code and its whole history, and copy it." },
          { text: "Make sure no commit holds passwords, keys or private files — even ones deleted later stay in the history.", warn: true },
          ...(sw ? [{ text: `Its ${sw} ${swGo}.` }] : []),
          { text: "Its Actions logs become visible to everyone." },
          ...(forks ? [{ text: forkWords("private") }] : []),
        ]
      : [
          {
            text:
              facts.owner_kind === "org"
                ? `Only the people ${facts.owner} gives access to will be able to see it.`
                : "Only you and the people you invite will be able to see it.",
          },
          // A public repository's forks stay public, split off; an internal one's are split off, who can see them unsaid.
          ...(forks && facts.visibility === "public" ? [{ text: forkWords("public") }] : []),
          ...(forks && facts.visibility === "internal" ? [{ text: splitOff }] : []),
          ...(facts.has_pages ? [{ text: "Its GitHub Pages site stops being published unless the account has a paid plan." }] : []),
        ];

  const title = !facts || block || behind ? "Who can see this repository" : asked === "public" ? "Make this repository public?" : "Make this repository private?";
  const stopped = !!(block || behind);
  const state = facts ? (behind ? (facts.visibility === asked ? "already" : "internal") : block ? "blocked" : asked) : loadError ? "error" : "loading";

  return (
    <Modal open={open} onClose={close} title={title} closeOnBackdrop={!typed && !understood}>
      <div className="space-y-4 text-sm" data-visibility-dialog={state}>
        {!facts && !loadError && (
          <p role="status" className="flex items-center gap-2 text-zinc-400">
            <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading {owner}/{name} from GitHub…
          </p>
        )}
        {loadError && (
          <>
            <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
              {loadError}
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={close} autoFocus>
                Cancel
              </Button>
              <Button onClick={() => void read(asked)}>Try again</Button>
            </div>
          </>
        )}

        {facts && (
          <>
            <VisibilityNow facts={facts} />
            {behind ? (
              state === "already" ? (
                <p className="flex items-start gap-2 px-3 py-2 rounded-lg bg-sky-500/10 border border-sky-500/30 text-sky-200 text-xs" data-visibility-block>
                  <Info size={14} className="mt-0.5 shrink-0" />
                  <span className="min-w-0">{behind}</span>
                </p>
              ) : (
                <p className="flex items-start gap-2 px-3 py-2 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-200 text-xs" data-visibility-block>
                  <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                  <span className="min-w-0">{behind}</span>
                </p>
              )
            ) : block ? (
              <p className="flex items-start gap-2 px-3 py-2 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-200 text-xs" data-visibility-block>
                <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                <span className="min-w-0">{block}</span>
              </p>
            ) : (
              <>
                <Consequences lines={lines} />
                {asked === "public" && (
                  <Input
                    autoFocus
                    label={`To confirm, type ${owner}/${name}`}
                    aria-label={`Type ${owner}/${name} to confirm`}
                    value={typed}
                    onChange={(e) => setTyped(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && ready) void submit();
                    }}
                    disabled={busy}
                    spellCheck={false}
                    autoComplete="off"
                    placeholder={`${owner}/${name}`}
                    className="font-mono text-sm"
                  />
                )}
                {asked === "private" && sw && (
                  <label className="flex items-start gap-2.5 px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/60 text-zinc-200 cursor-pointer select-none">
                    {/* The step to take first has the focus: Make private waits for this tick. */}
                    <Checkbox checked={understood} onChange={setUnderstood} disabled={busy} className="mt-0.5" aria-label={`I understand its ${sw} ${swGo}`} autoFocus />
                    <span>
                      I understand its {sw} {swGo}
                    </span>
                  </label>
                )}
              </>
            )}

            {error && (
              <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
                {error}
              </p>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2">
              {stopped && state !== "already" && facts.admin ? (
                <button
                  type="button"
                  className="text-xs text-zinc-500 hover:text-zinc-300 hover:underline cursor-pointer inline-flex items-center gap-1"
                  onClick={() => void openUrl(settingsUrl(owner, name)).catch(() => undefined)}
                >
                  <ExternalLink size={12} /> Open its settings on GitHub
                </button>
              ) : (
                <span />
              )}
              <div className="flex gap-2 ml-auto">
                <Button variant="secondary" onClick={close} disabled={busy} autoFocus={(asked === "private" && !sw) || stopped}>
                  {stopped ? "Close" : "Cancel"}
                </Button>
                {!stopped && asked === "public" && (
                  <Button variant="danger" onClick={() => void submit()} disabled={!ready} data-confirm="make-public">
                    {busy ? <Loader2 size={14} className="animate-spin" /> : <Globe size={14} />}
                    {busy ? "Making it public…" : "Make public"}
                  </Button>
                )}
                {/* Red when it loses something for good: its stars and watchers. */}
                {!stopped && asked === "private" && (
                  <Button variant={losesForGood ? "danger" : "primary"} onClick={() => void submit()} disabled={!ready} data-confirm="make-private">
                    {busy ? <Loader2 size={14} className="animate-spin" /> : <Lock size={14} />}
                    {busy ? "Making it private…" : "Make private"}
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
