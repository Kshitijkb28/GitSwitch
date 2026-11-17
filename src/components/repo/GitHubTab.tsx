import { useEffect, useRef, useState } from "react";
import { AlertTriangle, Building2, CheckCircle2, ExternalLink, Globe, Loader2, Lock, RotateCcw } from "lucide-react";
import * as api from "../../lib/api";
import { usePersistedState } from "../../lib/persist";
import { lastChangeTo, onRepoChange, sameRepo, type Deleted } from "../../lib/repoChanges";
import { Button } from "../Button";
import { GitHubIcon } from "../GitHubIcon";
import { Select } from "../Select";
import { Collaborators } from "../github/Collaborators";
import { PullList } from "../github/PullList";
import { PullPanel } from "../github/PullPanel";
import { RepoSettings } from "../github/RepoSettings";
import { restoreWords } from "../github/repoAdmin";
import { Segmented, cleanError, openOnGitHub } from "../github/shared";

const PERMISSION_WORD: Record<api.AccountAccess["permission"], string> = {
  admin: "admin",
  push: "can push",
  read: "can read",
  none: "no access",
  unknown: "not checked",
};

type Visibility = api.RepoFacts["visibility"];

/** GitHub's form for a new repository, filled in with this owner/name: private, the careful way. */
function newRepoUrl(fullName: string): string {
  const [owner = "", name = ""] = fullName.split("/");
  return `https://github.com/new?owner=${encodeURIComponent(owner)}&name=${encodeURIComponent(name)}&visibility=private`;
}

/** Private, Public or Internal, beside the repository's name. */
function VisibilityBadge({ visibility }: { visibility: Visibility }) {
  const [icon, word, cls] =
    visibility === "public"
      ? [<Globe size={12} />, "Public", "bg-sky-500/10 text-sky-300 border-sky-500/30"]
      : visibility === "internal"
        ? [<Building2 size={12} />, "Internal", "bg-violet-500/10 text-violet-300 border-violet-500/30"]
        : [<Lock size={12} />, "Private", "bg-amber-500/10 text-amber-300 border-amber-500/30"];
  return (
    <span
      className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full border font-sans text-[11px] font-medium shrink-0 ${cls}`}
      data-visibility-badge={visibility}
      title={visibility === "public" ? "Anyone on the internet can see it" : visibility === "internal" ? "Everyone in its enterprise can see it" : "Only the people with access can see it"}
    >
      {icon}
      {word}
    </span>
  );
}

type Props = {
  repoPath: string;
  visible: boolean;
  reloadKey?: number;
  /**
   * Make "owner/name" again on GitHub, under the same name, so this folder's
   * address stays as it is (the Publish dialog, over this tab). Without it,
   * GitHub's own form is the way.
   */
  onCreateAgain?: (fullName: string) => void;
};

/**
 * The open repository on GitHub: its pull requests (to review and merge), who
 * it is shared with, and its settings — who can see it, deleting it — as one
 * of your accounts, the profile's own by default when it can see the
 * repository.
 */
export function GitHubTab({ repoPath, visible, reloadKey = 0, onCreateAgain }: Props) {
  const [ctx, setCtx] = useState<api.RepoContext | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The account picked per repository, by "owner/name". */
  const [picked, setPicked] = usePersistedState<Record<string, string>>("github.actingAs", {});
  const [savedSection, setSection] = usePersistedState<string>("github.section", "pulls");
  const section: "pulls" | "people" | "settings" = savedSection === "people" || savedSection === "settings" ? savedSection : "pulls";
  const [state, setState] = usePersistedState<api.PullState>("github.pullState", "open");
  const [open, setOpen] = useState<api.PullItem | null>(null);
  const [reload, setReload] = useState(0);
  /** Who can see it as GitHub said last (the Settings section reads it, a change sets it), by "owner/name". */
  const [seen, setSeen] = useState<{ slug: string; visibility: Visibility } | null>(null);
  /**
   * The repository deleted on GitHub from GitSwitch — from this tab, or
   * earlier in this session (the Repositories page, or this tab before the
   * page was left): shown calmly, with how to get it back — Refresh too keeps
   * it, rather than "pick an account that can see it" — until an account can
   * see a repository there again (made again, or restored on GitHub).
   */
  const [deleted, setDeleted] = useState<Deleted | null>(null);
  /** The repository the context was read for; a tab hidden since keeps it. */
  const readFor = useRef<string | null>(null);
  const ask = useRef(0);

  async function load() {
    const id = ++ask.current;
    readFor.current = repoPath;
    setLoading(true);
    setError(null);
    setSeen(null);
    try {
      const c = await api.githubRepoContext(repoPath);
      if (id !== ask.current) return;
      setCtx(c);
      const there = c.owner && c.name ? `${c.owner}/${c.name}` : null;
      if (there) {
        if (c.accounts.some((a) => a.permission === "admin" || a.permission === "push" || a.permission === "read")) {
          setDeleted((d) => (d && sameRepo(d.fullName, there) ? null : d));
        } else {
          // No account sees it, and the last thing GitSwitch did to it was to delete it.
          const last = lastChangeTo(there);
          if (last?.kind === "deleted") setDeleted({ fullName: last.fullName, message: last.message, restoreUrl: last.restoreUrl });
        }
      }
    } catch (e) {
      if (id === ask.current) {
        setCtx(null);
        setError(cleanError(e));
      }
    } finally {
      if (id === ask.current) setLoading(false);
    }
  }

  useEffect(() => {
    // Read only once the tab is shown, and again when another repository is picked.
    if (visible && repoPath && readFor.current !== repoPath) {
      setOpen(null);
      void load();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, repoPath]);

  // The deleted repository made again (the Publish dialog, over this tab): the
  // calm note goes, and the tab reads what GitHub has now — at once when it is
  // shown, else on the next visit.
  const latest = useRef({ load, visible });
  latest.current = { load, visible };
  useEffect(() => {
    if (!deleted) return;
    return onRepoChange((change) => {
      if (change.kind !== "created" || !sameRepo(change.fullName, deleted.fullName)) return;
      setDeleted(null);
      if (latest.current.visible && readFor.current) void latest.current.load();
      else readFor.current = null;
    });
  }, [deleted]);

  // The page's Refresh: read the accounts and the list again (only once read before).
  const firstReload = useRef(reloadKey);
  useEffect(() => {
    if (reloadKey === firstReload.current) return;
    firstReload.current = reloadKey;
    if (visible && readFor.current) {
      void load();
      setReload((n) => n + 1);
    } else {
      readFor.current = null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reloadKey]);

  const slug = ctx?.owner && ctx.name ? `${ctx.owner}/${ctx.name}` : null;
  const accounts = ctx?.accounts ?? [];
  const chosen = (slug && accounts.find((a) => a.account === picked[slug.toLowerCase()])) || accounts.find((a) => a.account === ctx?.suggested) || accounts[0] || null;
  const account = chosen?.account ?? "";
  const gone = !!slug && !!deleted && sameRepo(deleted.fullName, slug);
  // GitHub's own answer once read (or changed here); before that, what the account's access check saw.
  const visibility: Visibility | null =
    slug && seen && seen.slug.toLowerCase() === slug.toLowerCase() ? seen.visibility : chosen?.private == null ? null : chosen.private ? "private" : "public";

  // Deleted from this tab: the dialog and the button that opened it are gone
  // with the Settings section, so the focus goes to the note that takes their place.
  const note = useRef<HTMLDivElement>(null);
  const focusNote = useRef(false);
  useEffect(() => {
    if (!gone || !focusNote.current) return;
    focusNote.current = false;
    note.current?.focus();
  }, [gone]);
  const restore = deleted ? restoreWords(deleted.message, deleted.fullName) : "";

  return (
    <div className={visible ? "space-y-4" : "hidden"}>
      {loading && !ctx && (
        <p className="flex items-center gap-2 text-sm text-zinc-400" role="status">
          <Loader2 size={14} className="animate-spin text-emerald-400" /> Asking GitHub which of your accounts can reach this repository…
        </p>
      )}
      {error && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs">
          {error}
        </p>
      )}
      {ctx?.not_github && <p className="text-sm text-zinc-400">{ctx.not_github} Pull requests and collaborators are for repositories on GitHub.</p>}

      {gone && deleted && (
        <div
          ref={note}
          tabIndex={-1}
          className="rounded-xl border border-zinc-700/50 bg-zinc-800/40 p-5 space-y-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500/40"
          role="status"
          data-deleted-on-github={deleted.fullName}
        >
          <p className="flex items-start gap-2 text-sm text-zinc-200">
            <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-emerald-400" />
            <span className="min-w-0">
              <span className="font-mono break-all">{deleted.fullName}</span> was deleted on GitHub.
            </span>
          </p>
          <p className="text-sm text-zinc-400">
            This folder and its commits are still here. Its address still points to <span className="font-mono text-zinc-300 break-all">{deleted.fullName}</span>, so
            Push and Fetch fail until a repository exists there again.
          </p>
          {/* Deleted by mistake: GitHub's own restore is the undo, and only github.com offers it. */}
          {restore && (
            <div className="space-y-1.5" data-restore-note>
              <p className="text-sm text-zinc-400">
                {restore}
                {deleted.restoreUrl && " Restoring it brings back its issues and pull requests too."}
              </p>
              {deleted.restoreUrl && (
                <button
                  type="button"
                  className="text-xs text-zinc-400 hover:text-zinc-200 hover:underline cursor-pointer inline-flex items-center gap-1"
                  onClick={() => openOnGitHub(deleted.restoreUrl!)}
                  data-restore-link
                >
                  <ExternalLink size={12} /> Restore it on github.com
                </button>
              )}
            </div>
          )}
          {onCreateAgain ? (
            <>
              <p className="text-sm text-zinc-400">
                {restore ? "Or create it again" : "Create it again"} under the same name: a new, empty repository that this folder publishes to as before. Its
                old issues and pull requests don't come back{deleted.restoreUrl ? ", and GitHub can't restore the old one while this one has its name" : ""}.
              </p>
              <Button size="sm" variant="secondary" onClick={() => onCreateAgain(deleted.fullName)} data-create-again={deleted.fullName}>
                <RotateCcw size={14} />
                Create it again on GitHub…
              </Button>
            </>
          ) : (
            // GitHub's own form, filled in, where the app can't make it again itself.
            <button
              type="button"
              className="text-xs text-zinc-400 hover:text-zinc-200 hover:underline cursor-pointer inline-flex items-center gap-1"
              onClick={() => openOnGitHub(newRepoUrl(deleted.fullName))}
              title="Opens GitHub's form for a new repository with this name, private. Create it empty, then push from the Changes tab."
            >
              <ExternalLink size={12} /> Create it again on github.com
            </button>
          )}
        </div>
      )}

      {slug && !gone && (
        <>
          <div className="flex items-center gap-3 flex-wrap">
            <span className="flex items-center gap-2 text-sm text-zinc-200 font-mono min-w-0">
              <GitHubIcon size={15} />
              <span className="truncate" title={slug}>
                {slug}
              </span>
              {visibility && <VisibilityBadge visibility={visibility} />}
            </span>
            <Button variant="ghost" size="sm" onClick={() => openOnGitHub(`https://github.com/${slug}`)} title="Open on GitHub">
              <ExternalLink size={13} />
            </Button>
            <span className="text-xs text-zinc-500 ml-auto">Acting as</span>
            <div className="w-80 max-w-full">
              <Select
                value={account}
                onChange={(v) => setPicked((m) => ({ ...m, [slug.toLowerCase()]: v }))}
                placeholder={accounts.length ? "Pick an account" : "No GitHub account signed in"}
                optionIcon={<GitHubIcon size={14} />}
                options={accounts.map((a) => ({ value: a.account, label: `${a.login} · ${PERMISSION_WORD[a.permission]}${ctx?.profile_login?.toLowerCase() === a.login.toLowerCase() ? " · this profile" : ""}` }))}
              />
            </div>
          </div>
          {accounts.length === 0 && <p className="text-sm text-amber-300/90">Sign in on the GitHub Auth page (or with `gh auth login`) to see this repository's pull requests.</p>}
          {chosen && (chosen.permission === "none" || chosen.permission === "unknown") && (
            <p className="text-xs text-amber-300/90 flex items-center gap-2">
              <AlertTriangle size={13} className="shrink-0" />
              {chosen.permission === "none" ? `GitHub shows ${slug} to @${chosen.login} as not there: pick an account that can see it.` : `@${chosen.login}'s access could not be checked${chosen.why ? `: ${chosen.why}` : ""}.`}
            </p>
          )}

          {account && (
            <>
              <Segmented
                label="Show"
                value={section}
                onChange={setSection}
                options={[
                  { value: "pulls", label: "Pull requests" },
                  { value: "people", label: "Collaborators" },
                  { value: "settings", label: "Settings" },
                ]}
              />
              {section === "pulls" ? (
                <PullList account={account} source={{ kind: "repo", owner: ctx!.owner!, name: ctx!.name! }} state={state} onState={setState} onOpen={setOpen} reloadKey={reload} />
              ) : section === "people" ? (
                <Collaborators account={account} owner={ctx!.owner!} name={ctx!.name!} admin={chosen?.permission === "admin"} />
              ) : (
                <RepoSettings
                  account={account}
                  owner={ctx!.owner!}
                  name={ctx!.name!}
                  repoPath={repoPath}
                  reloadKey={reload}
                  onVisibility={(v) => setSeen({ slug, visibility: v })}
                  onDeleted={(d) => {
                    setOpen(null);
                    focusNote.current = true;
                    setDeleted(d);
                  }}
                />
              )}
            </>
          )}
        </>
      )}

      {open && account && <PullPanel account={account} repo={open.repo} number={open.number} onClose={() => setOpen(null)} onChanged={() => setReload((n) => n + 1)} />}
    </div>
  );
}
